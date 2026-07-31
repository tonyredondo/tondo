use std::collections::BTreeMap;
use std::io::{self, Read};
use std::process::{Child, ChildStderr, ChildStdout, Command as OsCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};

use tondo_vm::runtime::{RuntimeHostValueKind, RuntimeValue, VmError, VmHost};

#[derive(Clone)]
struct ProcessStage {
    program: String,
    arguments: Vec<String>,
}

#[derive(Clone)]
struct ProcessPlan {
    stages: Vec<ProcessStage>,
}

#[derive(Debug, Clone)]
struct ExitStatus {
    code: Option<i32>,
    success: bool,
    downstream_closed_pipe: bool,
}

#[derive(Debug, Clone)]
struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    statuses: Vec<ExitStatus>,
}

enum HostValue {
    Command(ProcessPlan),
    Pipeline(ProcessPlan),
    Bytes(Vec<u8>),
    ExitStatus(ExitStatus),
    ProcessOutput(ProcessOutput),
    ProcessHandle(ProcessGroup),
    ProcessError { _message: String },
    ProcessExitError { _output: ProcessOutput },
    Utf8Error { _message: String },
    BytesBuilder(Vec<u8>),
    BytesError { _message: String },
}

#[derive(Debug, Clone, Copy)]
enum CompletionMode {
    Status,
    Output,
    Run,
    Check,
    Cancel,
}

struct AsyncJob {
    receiver: mpsc::Receiver<Result<ProcessOutput, String>>,
    cancellation: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    mode: CompletionMode,
}

pub(crate) struct BootstrapHost {
    pub(crate) stdout: Vec<u8>,
    arguments: Vec<String>,
    values: BTreeMap<u64, HostValue>,
    jobs: BTreeMap<u64, AsyncJob>,
    next_value: u64,
    next_job: u64,
    max_bytes: u64,
}

impl BootstrapHost {
    pub(crate) fn new(arguments: Vec<String>) -> Self {
        Self::with_max_bytes(arguments, u64::MAX)
    }

    pub(crate) fn with_max_bytes(arguments: Vec<String>, max_bytes: u64) -> Self {
        Self {
            stdout: Vec::new(),
            arguments,
            values: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_value: 0,
            next_job: 0,
            max_bytes,
        }
    }

    pub(crate) fn take_stdout(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.stdout)
    }

    fn allocate(&mut self, kind: RuntimeHostValueKind, value: HostValue) -> RuntimeValue {
        let id = self.next_value;
        self.next_value = self
            .next_value
            .checked_add(1)
            .expect("host value identity space exhausted");
        self.values.insert(id, value);
        RuntimeValue::Host { kind, id }
    }

    fn process_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::ProcessError,
            HostValue::ProcessError {
                _message: message.into(),
            },
        )
    }

    fn result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.process_error(message)))
    }

    fn bytes_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::BytesError,
            HostValue::BytesError {
                _message: message.into(),
            },
        )
    }

    fn bytes_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.bytes_error(message)))
    }

    fn ensure_bytes_len(&self, length: usize) -> Result<(), String> {
        if u64::try_from(length).is_ok_and(|length| length <= self.max_bytes) {
            Ok(())
        } else {
            Err(format!(
                "byte buffer length {length} exceeds the configured limit {}",
                self.max_bytes
            ))
        }
    }

    fn plan(&self, value: &RuntimeValue) -> Result<ProcessPlan, VmError> {
        let RuntimeValue::Host { kind, id } = value else {
            return Err(VmError::Host("process plan is not a host value".into()));
        };
        match (kind, self.values.get(id)) {
            (RuntimeHostValueKind::Command, Some(HostValue::Command(plan)))
            | (RuntimeHostValueKind::Pipeline, Some(HostValue::Pipeline(plan))) => Ok(plan.clone()),
            _ => Err(VmError::Host("process plan token is invalid".into())),
        }
    }

    fn bytes(&self, value: &RuntimeValue) -> Result<&[u8], VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Bytes,
            id,
        } = value
        else {
            return Err(VmError::Host("Bytes receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Bytes(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("Bytes token is stale".into())),
        }
    }

    fn builder(&self, value: &RuntimeValue) -> Result<&[u8], VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::BytesBuilder,
            id,
        } = value
        else {
            return Err(VmError::Host("BytesBuilder receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::BytesBuilder(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("BytesBuilder token is stale".into())),
        }
    }

    fn builder_mut(&mut self, value: &RuntimeValue) -> Result<&mut Vec<u8>, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::BytesBuilder,
            id,
        } = value
        else {
            return Err(VmError::Host("BytesBuilder receiver is invalid".into()));
        };
        match self.values.get_mut(id) {
            Some(HostValue::BytesBuilder(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("BytesBuilder token is stale".into())),
        }
    }

    fn array_bytes(value: &RuntimeValue) -> Result<Vec<u8>, VmError> {
        let RuntimeValue::Array(values) = value else {
            return Err(VmError::Host("expected Array[Byte]".into()));
        };
        values
            .iter()
            .map(|value| match value {
                RuntimeValue::Byte(value) => Ok(*value),
                _ => Err(VmError::Host("expected Array[Byte] element".into())),
            })
            .collect()
    }

    fn bytes_array(bytes: &[u8]) -> RuntimeValue {
        RuntimeValue::Array(bytes.iter().copied().map(RuntimeValue::Byte).collect())
    }

    fn output(&self, value: &RuntimeValue) -> Result<ProcessOutput, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::ProcessOutput,
            id,
        } = value
        else {
            return Err(VmError::Host("ProcessOutput receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProcessOutput(output)) => Ok(output.clone()),
            _ => Err(VmError::Host("ProcessOutput token is stale".into())),
        }
    }

    fn status(&self, value: &RuntimeValue) -> Result<ExitStatus, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::ExitStatus,
            id,
        } = value
        else {
            return Err(VmError::Host("ExitStatus receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ExitStatus(status)) => Ok(status.clone()),
            _ => Err(VmError::Host("ExitStatus token is stale".into())),
        }
    }

    fn allocate_statuses(&mut self, statuses: Vec<ExitStatus>) -> RuntimeValue {
        RuntimeValue::Array(
            statuses
                .into_iter()
                .map(|status| {
                    self.allocate(
                        RuntimeHostValueKind::ExitStatus,
                        HostValue::ExitStatus(status),
                    )
                })
                .collect(),
        )
    }

    fn complete_output(
        &mut self,
        mode: CompletionMode,
        result: Result<ProcessOutput, String>,
    ) -> RuntimeValue {
        let output = match result {
            Ok(output) => output,
            Err(message) => return self.result_error(message),
        };
        if let Err(message) = self.ensure_bytes_len(output.stdout.len()) {
            return self.result_error(format!("process stdout exceeds byte limit: {message}"));
        }
        if let Err(message) = self.ensure_bytes_len(output.stderr.len()) {
            return self.result_error(format!("process stderr exceeds byte limit: {message}"));
        }
        match mode {
            CompletionMode::Status | CompletionMode::Run | CompletionMode::Cancel => {
                RuntimeValue::ResultOk(Box::new(self.allocate_statuses(output.statuses)))
            }
            CompletionMode::Output => {
                let output = self.allocate(
                    RuntimeHostValueKind::ProcessOutput,
                    HostValue::ProcessOutput(output),
                );
                RuntimeValue::ResultOk(Box::new(output))
            }
            CompletionMode::Check if check_succeeded(&output.statuses) => {
                let output = self.allocate(
                    RuntimeHostValueKind::ProcessOutput,
                    HostValue::ProcessOutput(output),
                );
                RuntimeValue::ResultOk(Box::new(output))
            }
            CompletionMode::Check => {
                let error = self.allocate(
                    RuntimeHostValueKind::ProcessExitError,
                    HostValue::ProcessExitError { _output: output },
                );
                RuntimeValue::ResultErr(Box::new(error))
            }
        }
    }

    fn spawn_job(
        &mut self,
        group: Result<ProcessGroup, ProcessPlan>,
        mode: CompletionMode,
    ) -> Result<u64, VmError> {
        let cancellation = match &group {
            Ok(group) => Arc::clone(&group.cancellation),
            Err(_) => Arc::new(AtomicBool::new(false)),
        };
        if matches!(mode, CompletionMode::Cancel) {
            cancellation.store(true, Ordering::Release);
        }
        let worker_cancellation = Arc::clone(&cancellation);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("tondo-process".into())
            .spawn(move || {
                let result = match group {
                    Ok(group) => group.finish(),
                    Err(plan) => ProcessGroup::spawn(&plan, worker_cancellation)
                        .and_then(ProcessGroup::finish),
                };
                let _ = sender.send(result);
            })
            .map_err(|error| VmError::Host(format!("cannot start process worker: {error}")))?;
        let id = self.next_job;
        self.next_job = self
            .next_job
            .checked_add(1)
            .ok_or_else(|| VmError::Host("async host identity space exhausted".into()))?;
        self.jobs.insert(
            id,
            AsyncJob {
                receiver,
                cancellation,
                worker: Some(worker),
                mode,
            },
        );
        Ok(id)
    }

    fn finish_job(
        &mut self,
        id: u64,
        result: Result<ProcessOutput, String>,
    ) -> Result<RuntimeValue, VmError> {
        let mut job = self
            .jobs
            .remove(&id)
            .ok_or_else(|| VmError::Host(format!("unknown async host call #{id}")))?;
        if let Some(worker) = job.worker.take() {
            worker
                .join()
                .map_err(|_| VmError::Host("process worker panicked".into()))?;
        }
        Ok(self.complete_output(job.mode, result))
    }

    fn mode(name: &str) -> Option<CompletionMode> {
        Some(match name.rsplit('.').next()? {
            "status" => CompletionMode::Status,
            "output" => CompletionMode::Output,
            "run" => CompletionMode::Run,
            "check" => CompletionMode::Check,
            "cancel" => CompletionMode::Cancel,
            _ => return None,
        })
    }
}

impl Default for BootstrapHost {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl VmHost for BootstrapHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        match (name, arguments) {
            ("std.console.print", [RuntimeValue::String(text)]) => {
                self.stdout.extend_from_slice(text.as_bytes());
                Ok(RuntimeValue::Unit)
            }
            ("std.bytes.empty", []) => Ok(RuntimeValue::ResultOk(Box::new(
                self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(Vec::new())),
            ))),
            ("intrinsic.Bytes.fromString", [RuntimeValue::String(text)]) => {
                if let Err(message) = self.ensure_bytes_len(text.len()) {
                    return Ok(self.bytes_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(text.as_bytes().to_vec()),
                ))))
            }
            ("std.bytes.fromArray", [array]) => {
                let bytes = Self::array_bytes(array)?;
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.bytes_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes),
                ))))
            }
            ("std.bytes.builder", []) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                RuntimeHostValueKind::BytesBuilder,
                HostValue::BytesBuilder(Vec::new()),
            )))),
            ("std.bytes.Bytes.length", [receiver]) => Ok(RuntimeValue::Integer(
                i128::try_from(self.bytes(receiver)?.len())
                    .map_err(|_| VmError::Host("Bytes length does not fit in Int".into()))?,
            )),
            ("std.bytes.Bytes.get", [receiver, RuntimeValue::Integer(index)]) => {
                let bytes = self.bytes(receiver)?;
                let Some(index) = usize::try_from(*index).ok() else {
                    return Ok(RuntimeValue::OptionNone);
                };
                Ok(bytes
                    .get(index)
                    .copied()
                    .map(|byte| RuntimeValue::OptionSome(Box::new(RuntimeValue::Byte(byte))))
                    .unwrap_or(RuntimeValue::OptionNone))
            }
            (
                "std.bytes.Bytes.slice",
                [
                    receiver,
                    RuntimeValue::Integer(start),
                    RuntimeValue::Integer(end),
                ],
            ) => {
                let bytes = self.bytes(receiver)?.to_vec();
                let Some(start) = usize::try_from(*start).ok() else {
                    return Ok(self.bytes_result_error("slice start must be non-negative"));
                };
                let Some(end) = usize::try_from(*end).ok() else {
                    return Ok(self.bytes_result_error("slice end must be non-negative"));
                };
                if start > end || end > bytes.len() {
                    return Ok(self.bytes_result_error(format!(
                        "slice [{start}, {end}) is outside a buffer of length {}",
                        bytes.len()
                    )));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes[start..end].to_vec()),
                ))))
            }
            ("std.bytes.Bytes.toArray", [receiver]) => {
                let bytes = self.bytes(receiver)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.bytes_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(Self::bytes_array(&bytes))))
            }
            ("intrinsic.String.fromBytes", [receiver]) => {
                match String::from_utf8(self.bytes(receiver)?.to_vec()) {
                    Ok(text) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(text)))),
                    Err(error) => {
                        let error = self.allocate(
                            RuntimeHostValueKind::Utf8Error,
                            HostValue::Utf8Error {
                                _message: error.to_string(),
                            },
                        );
                        Ok(RuntimeValue::ResultErr(Box::new(error)))
                    }
                }
            }
            ("std.bytes.Bytes.equal", [left, right]) => {
                Ok(RuntimeValue::Bool(self.bytes(left)? == self.bytes(right)?))
            }
            ("std.bytes.Bytes.hash", [receiver]) => {
                let mut hash = 14_695_981_039_346_656_037_u64;
                for byte in self.bytes(receiver)? {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(1_099_511_628_211);
                }
                Ok(RuntimeValue::Integer(i128::from(hash)))
            }
            ("std.bytes.BytesBuilder.length", [receiver]) => Ok(RuntimeValue::Integer(
                i128::try_from(self.builder(receiver)?.len())
                    .map_err(|_| VmError::Host("BytesBuilder length does not fit in Int".into()))?,
            )),
            ("std.bytes.BytesBuilder.appendByte", [receiver, RuntimeValue::Byte(byte)]) => {
                let current = self.builder(receiver)?.len();
                let Some(length) = current.checked_add(1) else {
                    return Ok(self.bytes_result_error("BytesBuilder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.bytes_result_error(message));
                }
                self.builder_mut(receiver)?.push(*byte);
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.bytes.BytesBuilder.append", [receiver, bytes]) => {
                let appended = self.bytes(bytes)?.to_vec();
                let current = self.builder(receiver)?.len();
                let Some(length) = current.checked_add(appended.len()) else {
                    return Ok(self.bytes_result_error("BytesBuilder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.bytes_result_error(message));
                }
                self.builder_mut(receiver)?.extend_from_slice(&appended);
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.bytes.BytesBuilder.appendArray", [receiver, array]) => {
                let appended = Self::array_bytes(array)?;
                let current = self.builder(receiver)?.len();
                let Some(length) = current.checked_add(appended.len()) else {
                    return Ok(self.bytes_result_error("BytesBuilder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.bytes_result_error(message));
                }
                self.builder_mut(receiver)?.extend_from_slice(&appended);
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.bytes.BytesBuilder.finish", [receiver]) => {
                let bytes = self.builder(receiver)?.to_vec();
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes),
                ))))
            }
            ("std.process.args", []) => Ok(RuntimeValue::Array(
                self.arguments
                    .iter()
                    .cloned()
                    .map(RuntimeValue::String)
                    .collect(),
            )),
            (
                "std.process.cmd",
                [
                    RuntimeValue::String(program),
                    RuntimeValue::Array(arguments),
                ],
            ) => {
                let arguments = arguments
                    .iter()
                    .map(|argument| match argument {
                        RuntimeValue::String(argument) => Ok(argument.clone()),
                        _ => Err(VmError::Host(
                            "std.process.cmd received a non-String argument".into(),
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.allocate(
                    RuntimeHostValueKind::Command,
                    HostValue::Command(ProcessPlan {
                        stages: vec![ProcessStage {
                            program: program.clone(),
                            arguments,
                        }],
                    }),
                ))
            }
            ("std.process.shell", [RuntimeValue::String(text)]) => Ok(self.allocate(
                RuntimeHostValueKind::Command,
                HostValue::Command(ProcessPlan {
                    stages: vec![shell_stage(text)],
                }),
            )),
            ("std.process.pipe", [left, right]) => {
                let mut plan = self.plan(left)?;
                plan.stages.extend(self.plan(right)?.stages);
                Ok(self.allocate(RuntimeHostValueKind::Pipeline, HostValue::Pipeline(plan)))
            }
            ("std.process.Command.start" | "std.process.Pipeline.start", [receiver]) => {
                match ProcessGroup::spawn(&self.plan(receiver)?, Arc::new(AtomicBool::new(false))) {
                    Ok(group) => {
                        let handle = self.allocate(
                            RuntimeHostValueKind::ProcessHandle,
                            HostValue::ProcessHandle(group),
                        );
                        Ok(RuntimeValue::ResultOk(Box::new(handle)))
                    }
                    Err(message) => Ok(self.result_error(message)),
                }
            }
            ("std.process.ProcessOutput.stdout", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output.stdout)))
            }
            ("std.process.ProcessOutput.stderr", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output.stderr)))
            }
            ("std.process.ProcessOutput.statuses", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate_statuses(output.statuses))
            }
            ("std.process.ExitStatus.code", [receiver]) => Ok(match self.status(receiver)?.code {
                Some(code) => {
                    RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(i128::from(code))))
                }
                None => RuntimeValue::OptionNone,
            }),
            ("std.process.ExitStatus.success", [receiver]) => {
                Ok(RuntimeValue::Bool(self.status(receiver)?.success))
            }
            ("std.console.print", _) => Err(VmError::Host(
                "std.console.print received an invalid bootstrap argument list".into(),
            )),
            _ => Err(VmError::UnsupportedHostCall(name.to_owned())),
        }
    }

    fn start_async(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<u64, VmError> {
        let mode = Self::mode(name).ok_or_else(|| VmError::UnsupportedHostCall(name.to_owned()))?;
        let [receiver] = arguments else {
            return Err(VmError::Host(format!(
                "{name} received an invalid bootstrap argument list"
            )));
        };
        let group = match receiver {
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::ProcessHandle,
                id,
            } => match self.values.remove(id) {
                Some(HostValue::ProcessHandle(group)) => Ok(group),
                _ => return Err(VmError::Host("ProcessHandle token is stale".into())),
            },
            _ => Err(self.plan(receiver)?),
        };
        self.spawn_job(group, mode)
    }

    fn poll_async(&mut self, call: u64) -> Result<Option<RuntimeValue>, VmError> {
        let result = {
            let job = self
                .jobs
                .get(&call)
                .ok_or_else(|| VmError::Host(format!("unknown async host call #{call}")))?;
            match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(VmError::Host("process worker disconnected".into()));
                }
            }
        };
        result
            .map(|result| self.finish_job(call, result))
            .transpose()
    }

    fn wait_async(&mut self, calls: &[u64]) -> Result<(u64, RuntimeValue), VmError> {
        if calls.is_empty() {
            return Err(VmError::Host("host wait received no process calls".into()));
        }
        loop {
            for call in calls {
                if let Some(value) = self.poll_async(*call)? {
                    return Ok((*call, value));
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn cancel_async(&mut self, call: u64) -> Result<(), VmError> {
        let job = self
            .jobs
            .get(&call)
            .ok_or_else(|| VmError::Host(format!("unknown async host call #{call}")))?;
        job.cancellation.store(true, Ordering::Release);
        Ok(())
    }

    fn cleanup(&mut self, value: &RuntimeValue) -> Result<(), VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::ProcessHandle,
            id,
        } = value
        else {
            return Ok(());
        };
        self.values.remove(id);
        Ok(())
    }
}

impl Drop for BootstrapHost {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancellation.store(true, Ordering::Release);
        }
        for (_, mut job) in std::mem::take(&mut self.jobs) {
            if let Some(worker) = job.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

struct ProcessGroup {
    children: Vec<Child>,
    stdout: Option<JoinHandle<io::Result<Vec<u8>>>>,
    stderr: Vec<JoinHandle<io::Result<Vec<u8>>>>,
    cancellation: Arc<AtomicBool>,
}

impl ProcessGroup {
    fn spawn(plan: &ProcessPlan, cancellation: Arc<AtomicBool>) -> Result<Self, String> {
        if plan.stages.is_empty() {
            return Err("cannot execute an empty process plan".into());
        }
        let mut children = Vec::with_capacity(plan.stages.len());
        let mut stderr = Vec::with_capacity(plan.stages.len());
        let mut previous_stdout: Option<ChildStdout> = None;
        let mut final_stdout = None;

        for (index, stage) in plan.stages.iter().enumerate() {
            let final_stage = index + 1 == plan.stages.len();
            let mut command = OsCommand::new(&stage.program);
            command.args(&stage.arguments);
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            configure_process_group(&mut command);
            if let Some(stdout) = previous_stdout.take() {
                command.stdin(Stdio::from(stdout));
            }
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    cleanup_partial(&mut children, &mut stderr);
                    return Err(format!("cannot spawn `{}`: {error}", stage.program));
                }
            };
            let Some(stdout) = child.stdout.take() else {
                terminate(&mut child);
                let _ = child.wait();
                cleanup_partial(&mut children, &mut stderr);
                return Err(format!("cannot capture stdout for `{}`", stage.program));
            };
            let Some(child_stderr) = child.stderr.take() else {
                terminate(&mut child);
                let _ = child.wait();
                cleanup_partial(&mut children, &mut stderr);
                return Err(format!("cannot capture stderr for `{}`", stage.program));
            };
            children.push(child);
            let stderr_reader = match read_stderr(child_stderr) {
                Ok(reader) => reader,
                Err(error) => {
                    cleanup_partial(&mut children, &mut stderr);
                    return Err(format!(
                        "cannot start stderr reader for `{}`: {error}",
                        stage.program
                    ));
                }
            };
            stderr.push(stderr_reader);
            if final_stage {
                final_stdout = match read_stdout(stdout) {
                    Ok(reader) => Some(reader),
                    Err(error) => {
                        cleanup_partial(&mut children, &mut stderr);
                        return Err(format!(
                            "cannot start stdout reader for `{}`: {error}",
                            stage.program
                        ));
                    }
                };
            } else {
                previous_stdout = Some(stdout);
            }
        }

        Ok(Self {
            children,
            stdout: final_stdout,
            stderr,
            cancellation,
        })
    }

    fn finish(mut self) -> Result<ProcessOutput, String> {
        let mut statuses = vec![None; self.children.len()];
        let mut termination_requested = false;
        while statuses.iter().any(Option::is_none) {
            if self.cancellation.load(Ordering::Acquire) && !termination_requested {
                termination_requested = true;
                for (child, status) in self.children.iter_mut().zip(&statuses) {
                    if status.is_none() {
                        terminate(child);
                    }
                }
            }
            for (child, status) in self.children.iter_mut().zip(&mut statuses) {
                if status.is_none() {
                    *status = child
                        .try_wait()
                        .map_err(|error| format!("cannot inspect child status: {error}"))?;
                }
            }
            if statuses.iter().any(Option::is_none) {
                thread::sleep(Duration::from_millis(2));
            }
        }

        let stdout = join_reader(self.stdout.take(), "stdout")?;
        let mut combined_stderr = Vec::new();
        for reader in self.stderr.drain(..) {
            combined_stderr.extend(join_reader(Some(reader), "stderr")?);
        }
        let statuses = statuses
            .into_iter()
            .map(|status| {
                let status = status.expect("status loop waits for every child");
                ExitStatus {
                    code: status.code(),
                    success: status.success(),
                    downstream_closed_pipe: downstream_closed_pipe(&status),
                }
            })
            .collect();
        Ok(ProcessOutput {
            stdout,
            stderr: combined_stderr,
            statuses,
        })
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        for child in &mut self.children {
            terminate(child);
        }
        for child in &mut self.children {
            let _ = child.wait();
        }
        if let Some(reader) = self.stdout.take() {
            let _ = reader.join();
        }
        for reader in self.stderr.drain(..) {
            let _ = reader.join();
        }
    }
}

fn cleanup_partial(children: &mut [Child], stderr: &mut Vec<JoinHandle<io::Result<Vec<u8>>>>) {
    for child in children.iter_mut() {
        terminate(child);
    }
    for child in children.iter_mut() {
        let _ = child.wait();
    }
    for reader in stderr.drain(..) {
        let _ = reader.join();
    }
}

fn read_stdout(mut stream: ChildStdout) -> io::Result<JoinHandle<io::Result<Vec<u8>>>> {
    thread::Builder::new()
        .name("tondo-process-stdout".into())
        .spawn(move || read_all(&mut stream))
}

fn read_stderr(mut stream: ChildStderr) -> io::Result<JoinHandle<io::Result<Vec<u8>>>> {
    thread::Builder::new()
        .name("tondo-process-stderr".into())
        .spawn(move || read_all(&mut stream))
}

fn read_all(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    stream.read_to_end(&mut output)?;
    Ok(output)
}

fn join_reader(
    reader: Option<JoinHandle<io::Result<Vec<u8>>>>,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .ok_or_else(|| format!("{stream} reader is missing"))?
        .join()
        .map_err(|_| format!("{stream} reader panicked"))?
        .map_err(|error| format!("cannot read process {stream}: {error}"))
}

fn shell_stage(text: &str) -> ProcessStage {
    #[cfg(unix)]
    {
        ProcessStage {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), text.into()],
        }
    }
    #[cfg(windows)]
    {
        ProcessStage {
            program: "cmd.exe".into(),
            arguments: vec!["/C".into(), text.into()],
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        ProcessStage {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), text.into()],
        }
    }
}

fn check_succeeded(statuses: &[ExitStatus]) -> bool {
    let mut has_satisfactory_downstream = false;
    for status in statuses.iter().rev() {
        if status.success {
            has_satisfactory_downstream = true;
        } else if !(status.downstream_closed_pipe && has_satisfactory_downstream) {
            return false;
        }
    }
    has_satisfactory_downstream
}

fn downstream_closed_pipe(status: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        status.signal() == Some(13)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

fn configure_process_group(command: &mut OsCommand) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

fn terminate(child: &mut Child) {
    if child.try_wait().is_ok_and(|status| status.is_some()) {
        return;
    }
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let killed = OsCommand::new("/bin/kill")
            .args(["-KILL", "--", group.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !killed {
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::*;

    fn command(host: &mut BootstrapHost, program: &str, arguments: &[&str]) -> RuntimeValue {
        host.invoke(
            "std.process.cmd",
            &[
                RuntimeValue::String(program.into()),
                RuntimeValue::Array(
                    arguments
                        .iter()
                        .map(|argument| RuntimeValue::String((*argument).into()))
                        .collect(),
                ),
            ],
        )
        .unwrap()
    }

    fn shell(host: &mut BootstrapHost, text: &str) -> RuntimeValue {
        host.invoke("std.process.shell", &[RuntimeValue::String(text.into())])
            .unwrap()
    }

    fn pipe(host: &mut BootstrapHost, left: RuntimeValue, right: RuntimeValue) -> RuntimeValue {
        host.invoke("std.process.pipe", &[left, right]).unwrap()
    }

    fn await_call(host: &mut BootstrapHost, name: &str, receiver: RuntimeValue) -> RuntimeValue {
        let call = host.start_async(name, &[receiver]).unwrap();
        let (completed, value) = host.wait_async(&[call]).unwrap();
        assert_eq!(completed, call);
        value
    }

    fn ok(value: RuntimeValue) -> RuntimeValue {
        let RuntimeValue::ResultOk(value) = value else {
            panic!("expected successful process result");
        };
        *value
    }

    fn output_text(host: &mut BootstrapHost, output: RuntimeValue) -> String {
        let bytes = host
            .invoke("std.process.ProcessOutput.stdout", &[output])
            .unwrap();
        let text = host.invoke("intrinsic.String.fromBytes", &[bytes]).unwrap();
        let RuntimeValue::String(text) = ok(text) else {
            panic!("expected decoded process output");
        };
        text
    }

    fn output_stderr_text(host: &mut BootstrapHost, output: RuntimeValue) -> String {
        let bytes = host
            .invoke("std.process.ProcessOutput.stderr", &[output])
            .unwrap();
        let text = host.invoke("intrinsic.String.fromBytes", &[bytes]).unwrap();
        let RuntimeValue::String(text) = ok(text) else {
            panic!("expected decoded process stderr");
        };
        text
    }

    fn bytes_ok(value: RuntimeValue) -> RuntimeValue {
        let RuntimeValue::ResultOk(value) = value else {
            panic!("expected successful bytes result");
        };
        *value
    }

    #[test]
    fn bytes_are_immutable_values_with_copying_boundaries_and_stable_hashes() {
        let mut host = BootstrapHost::default();
        let source = bytes_ok(
            host.invoke(
                "intrinsic.Bytes.fromString",
                &[RuntimeValue::String("abc".into())],
            )
            .unwrap(),
        );
        let copy = bytes_ok(
            host.invoke("std.bytes.Bytes.toArray", std::slice::from_ref(&source))
                .unwrap(),
        );
        assert_eq!(
            copy,
            RuntimeValue::Array(vec![
                RuntimeValue::Byte(b'a'),
                RuntimeValue::Byte(b'b'),
                RuntimeValue::Byte(b'c'),
            ])
        );
        let round_trip = bytes_ok(
            host.invoke("std.bytes.fromArray", std::slice::from_ref(&copy))
                .unwrap(),
        );
        assert!(matches!(
            host.invoke(
                "std.bytes.Bytes.equal",
                &[source.clone(), round_trip.clone()]
            )
            .unwrap(),
            RuntimeValue::Bool(true)
        ));
        assert_eq!(
            host.invoke("std.bytes.Bytes.hash", std::slice::from_ref(&source))
                .unwrap(),
            host.invoke("std.bytes.Bytes.hash", std::slice::from_ref(&round_trip))
                .unwrap()
        );
        let slice = bytes_ok(
            host.invoke(
                "std.bytes.Bytes.slice",
                &[
                    source.clone(),
                    RuntimeValue::Integer(1),
                    RuntimeValue::Integer(3),
                ],
            )
            .unwrap(),
        );
        assert_eq!(
            ok(host.invoke("intrinsic.String.fromBytes", &[slice]).unwrap()),
            RuntimeValue::String("bc".into())
        );
        assert!(matches!(
            host.invoke(
                "std.bytes.Bytes.get",
                &[source.clone(), RuntimeValue::Integer(-1)]
            )
            .unwrap(),
            RuntimeValue::OptionNone
        ));
        assert!(matches!(
            host.invoke(
                "std.bytes.Bytes.get",
                &[source.clone(), RuntimeValue::Integer(99)]
            )
            .unwrap(),
            RuntimeValue::OptionNone
        ));
    }

    #[test]
    fn bytes_reject_invalid_utf8_and_ranges_without_partial_values() {
        let mut host = BootstrapHost::default();
        let invalid = bytes_ok(
            host.invoke(
                "std.bytes.fromArray",
                &[RuntimeValue::Array(vec![RuntimeValue::Byte(0xff)])],
            )
            .unwrap(),
        );
        let text = host
            .invoke("intrinsic.String.fromBytes", std::slice::from_ref(&invalid))
            .unwrap();
        assert!(matches!(text, RuntimeValue::ResultErr(value) if matches!(
            *value,
            RuntimeValue::Host { kind: RuntimeHostValueKind::Utf8Error, .. }
        )));
        for (start, end) in [(-1, 0), (2, 1), (0, 4)] {
            let result = host
                .invoke(
                    "std.bytes.Bytes.slice",
                    &[
                        invalid.clone(),
                        RuntimeValue::Integer(start),
                        RuntimeValue::Integer(end),
                    ],
                )
                .unwrap();
            assert!(matches!(result, RuntimeValue::ResultErr(value) if matches!(
                *value,
                RuntimeValue::Host { kind: RuntimeHostValueKind::BytesError, .. }
            )));
        }
    }

    #[test]
    fn bytes_builder_is_mutable_only_through_its_host_token_and_obeys_limits() {
        let mut host = BootstrapHost::with_max_bytes(Vec::new(), 3);
        let builder = bytes_ok(host.invoke("std.bytes.builder", &[]).unwrap());
        assert!(matches!(
            host.invoke(
                "std.bytes.BytesBuilder.appendArray",
                &[
                    builder.clone(),
                    RuntimeValue::Array(vec![
                        RuntimeValue::Byte(1),
                        RuntimeValue::Byte(2),
                        RuntimeValue::Byte(3),
                    ])
                ]
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if *value == RuntimeValue::Unit
        ));
        let rejected = host
            .invoke(
                "std.bytes.BytesBuilder.appendByte",
                &[builder.clone(), RuntimeValue::Byte(4)],
            )
            .unwrap();
        assert!(
            matches!(rejected, RuntimeValue::ResultErr(value) if matches!(
                *value,
                RuntimeValue::Host { kind: RuntimeHostValueKind::BytesError, .. }
            ))
        );
        assert_eq!(
            host.invoke(
                "std.bytes.BytesBuilder.length",
                std::slice::from_ref(&builder),
            )
            .unwrap(),
            RuntimeValue::Integer(3)
        );
        let finished = bytes_ok(
            host.invoke("std.bytes.BytesBuilder.finish", &[builder])
                .unwrap(),
        );
        assert!(matches!(
            host.invoke("std.bytes.Bytes.length", &[finished]).unwrap(),
            RuntimeValue::Integer(3)
        ));
        let mut limited = BootstrapHost::with_max_bytes(Vec::new(), 2);
        let rejected = limited
            .invoke(
                "intrinsic.Bytes.fromString",
                &[RuntimeValue::String("abc".into())],
            )
            .unwrap();
        assert!(
            matches!(rejected, RuntimeValue::ResultErr(value) if matches!(
                *value,
                RuntimeValue::Host { kind: RuntimeHostValueKind::BytesError, .. }
            ))
        );
    }

    #[test]
    fn plan_construction_is_inert_and_exact_arguments_bypass_the_shell() {
        let marker = std::env::temp_dir().join(format!(
            "tondo-inert-plan-{}-{}",
            std::process::id(),
            thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&marker);
        let mut host = BootstrapHost::default();
        let _plan = shell(&mut host, &format!("touch {}", marker.display()));
        assert!(!marker.exists());

        let exact = command(
            &mut host,
            "/usr/bin/printf",
            &["[%s][%s][%s]", "two words", "*", "$HOME"],
        );
        let output = ok(await_call(&mut host, "std.process.Command.output", exact));
        assert_eq!(output_text(&mut host, output), "[two words][*][$HOME]");
        assert!(!marker.exists());
    }

    #[test]
    fn all_four_pipe_shapes_preserve_stage_order() {
        let mut host = BootstrapHost::default();
        for shape in 0..4 {
            let source = command(&mut host, "/usr/bin/printf", &["pipe"]);
            let cat_one = command(&mut host, "/bin/cat", &[]);
            let cat_two = command(&mut host, "/bin/cat", &[]);
            let cat_three = command(&mut host, "/bin/cat", &[]);
            let pipeline = match shape {
                0 => pipe(&mut host, source, cat_one),
                1 => {
                    let right = pipe(&mut host, cat_one, cat_two);
                    pipe(&mut host, source, right)
                }
                2 => {
                    let left = pipe(&mut host, source, cat_one);
                    pipe(&mut host, left, cat_two)
                }
                3 => {
                    let left = pipe(&mut host, source, cat_one);
                    let right = pipe(&mut host, cat_two, cat_three);
                    pipe(&mut host, left, right)
                }
                _ => unreachable!(),
            };
            let output = ok(await_call(
                &mut host,
                "std.process.Pipeline.output",
                pipeline,
            ));
            assert_eq!(output_text(&mut host, output), "pipe");
        }

        let source = shell(&mut host, "printf left >&2; printf pipe");
        let sink = shell(&mut host, "cat >/dev/null; printf right >&2");
        let pipeline = pipe(&mut host, source, sink);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.output",
            pipeline,
        ));
        assert_eq!(output_stderr_text(&mut host, output), "leftright");
    }

    #[test]
    fn kernel_pipes_drain_output_larger_than_their_backpressure_window() {
        let mut host = BootstrapHost::default();
        let producer = shell(&mut host, "head -c 1048576 /dev/zero");
        let consumer = command(&mut host, "/usr/bin/wc", &["-c"]);
        let pipeline = pipe(&mut host, producer, consumer);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.output",
            pipeline,
        ));
        assert_eq!(output_text(&mut host, output).trim(), "1048576");

        let producer = command(&mut host, "/usr/bin/yes", &["bounded"]);
        let consumer = command(&mut host, "/usr/bin/head", &["-n", "1"]);
        let pipeline = pipe(&mut host, producer, consumer);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.check",
            pipeline,
        ));
        assert_eq!(output_text(&mut host, output), "bounded\n");
    }

    #[test]
    fn exit_status_is_data_while_check_and_spawn_failures_are_typed_errors() {
        let mut host = BootstrapHost::default();
        assert!(!check_succeeded(&[ExitStatus {
            code: None,
            success: false,
            downstream_closed_pipe: true,
        }]));

        let exits_seven = shell(&mut host, "exit 7");
        let status = ok(await_call(
            &mut host,
            "std.process.Command.status",
            exits_seven,
        ));
        let RuntimeValue::Array(statuses) = status else {
            panic!("expected status array");
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            host.invoke("std.process.ExitStatus.code", &statuses)
                .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(7)))
        );
        assert_eq!(
            host.invoke("std.process.ExitStatus.success", &statuses)
                .unwrap(),
            RuntimeValue::Bool(false)
        );

        let exits_seven = shell(&mut host, "exit 7");
        let checked = await_call(&mut host, "std.process.Command.check", exits_seven);
        assert!(matches!(
            checked,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::ProcessExitError,
                        ..
                    }
                )
        ));

        let succeeds = shell(&mut host, "exit 0");
        let exits_seven = shell(&mut host, "exit 7");
        let pipeline = pipe(&mut host, succeeds, exits_seven);
        let checked = await_call(&mut host, "std.process.Pipeline.check", pipeline);
        assert!(matches!(
            checked,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::ProcessExitError,
                        ..
                    }
                )
        ));

        let missing = command(&mut host, "/definitely/missing/tondo-process-test", &[]);
        let failed = await_call(&mut host, "std.process.Command.output", missing);
        assert!(matches!(
            failed,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::ProcessError,
                        ..
                    }
                )
        ));
    }

    #[test]
    fn bytes_to_string_rejects_invalid_utf8_without_replacement() {
        let mut host = BootstrapHost::default();
        let bytes = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0xf0, 0x28, 0x8c, 0x28]),
        );
        let decoded = host.invoke("intrinsic.String.fromBytes", &[bytes]).unwrap();
        assert!(matches!(
            decoded,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Utf8Error,
                        ..
                    }
                )
        ));
    }

    #[test]
    fn cancel_and_host_drop_reap_started_children() {
        let mut host = BootstrapHost::default();
        let plan = shell(&mut host, "exec sleep 30");
        let handle = ok(host.invoke("std.process.Command.start", &[plan]).unwrap());
        let started = Instant::now();
        let _ = ok(await_call(
            &mut host,
            "std.process.ProcessHandle.cancel",
            handle,
        ));
        assert!(started.elapsed() < Duration::from_secs(5));

        let plan = shell(&mut host, "exec sleep 30");
        let handle = ok(host.invoke("std.process.Command.start", &[plan]).unwrap());
        let RuntimeValue::Host { id, .. } = handle else {
            panic!("expected process handle");
        };
        let pid = match host.values.get(&id) {
            Some(HostValue::ProcessHandle(group)) => group.children[0].id(),
            _ => panic!("expected live process group"),
        };
        drop(host);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}
