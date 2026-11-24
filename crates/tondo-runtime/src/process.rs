//! `std.process` (standard library 19.2): plans of commands and pipelines,
//! the processes they start, and how their streams connect. A plan is an
//! immutable shared object, so `Command` and `Pipeline` copy by reference.

use super::io::{Mode, Ready, StreamObject, host_cancellable, replaced_console, retain, run, stream};
use super::poll::{Direction, Registered};
use super::tasks::{Wait, notify};
use super::{ArrayHeader, LIVE_OBJECTS, Str, element, str_bytes, tondo_rt_array_new};
use crate::array_items;
use crate::byte_array;
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;
const SIGPIPE: i32 = 13;

/// A stream of `std.io` a plan or a process holds a reference to.
struct Held(*mut StreamObject);

unsafe impl Send for Held {}
unsafe impl Sync for Held {}

impl Clone for Held {
    fn clone(&self) -> Held {
        Held(retain(self.0))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe { super::io::tondo_rt_stream_release(self.0) }
    }
}

#[derive(Clone)]
enum Input {
    Inherit,
    Null,
    Bytes(Arc<Vec<u8>>),
    File(PathBuf),
    Reader(Held),
    Pipe,
}

#[derive(Clone)]
enum Output {
    Inherit,
    Null,
    Capture,
    File(PathBuf),
    Append(PathBuf),
    Writer(Held),
    Stdout,
    Pipe,
}

#[derive(Clone)]
struct Stage {
    program: OsString,
    arguments: Vec<OsString>,
    directory: Option<PathBuf>,
    clear_env: bool,
    env: Vec<(OsString, Option<OsString>)>,
    stdin: Option<Input>,
    stdout: Option<Output>,
    stderr: Option<Output>,
    merge: bool,
    /// `group()`: the stages run in a process group of their own, which
    /// signals and `cancel` reach as a whole.
    group: bool,
    /// `grace(time)`: how long a cancelled process has between `SIGTERM`
    /// and `SIGKILL`, and its descendants to finish the output it captures.
    grace: Option<std::time::Duration>,
    /// `invokedAs(name)`: the program's `argv[0]`.
    invoked: Option<OsString>,
}

/// A `Command` (one stage) or a `Pipeline`.
#[repr(C)]
pub struct PlanObject {
    rc: AtomicUsize,
    stages: Vec<Stage>,
}

fn plan(stages: Vec<Stage>) -> *mut PlanObject {
    LIVE_OBJECTS.made();
    Box::into_raw(Box::new(PlanObject { rc: AtomicUsize::new(1), stages }))
}

fn os(text: *const Str) -> OsString {
    OsStr::from_bytes(unsafe { str_bytes(&text) }).to_owned()
}

fn stage(program: OsString, arguments: Vec<OsString>) -> Stage {
    Stage {
        program,
        arguments,
        directory: None,
        clear_env: false,
        env: Vec::new(),
        stdin: None,
        stdout: None,
        stderr: None,
        merge: false,
        group: false,
        grace: None,
        invoked: None,
    }
}

/// `process.command(program, arguments...)`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_command(program: *const Str, arguments: *mut ArrayHeader) -> *mut PlanObject {
    let arguments =
        (0..unsafe { (*arguments).len }).map(|i| os(unsafe { *(element(arguments, 8, i) as *const *const Str) }));
    plan(vec![stage(os(program), arguments.collect())])
}

/// A pipeline of the one stage of a command, as `process.parse` gives one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_alone(command: *mut PlanObject) -> *mut PlanObject {
    plan(unsafe { (*command).stages.clone() })
}

/// `process.shell(text)`: the system shell runs the text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_shell(text: *const Str) -> *mut PlanObject {
    plan(vec![stage("/bin/sh".into(), vec!["-c".into(), os(text)])])
}

/// `left | right`: the stages of both, in order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_pipe(left: *mut PlanObject, right: *mut PlanObject) -> *mut PlanObject {
    let stages = unsafe { (*left).stages.iter().chain((*right).stages.iter()).cloned().collect() };
    plan(stages)
}

/// A copy of the plan changed by one setting: 0 `directory(a)`, 1
/// `env(a, b)`, 2 `removeEnv(a)`, 3 `clearEnv()`, 4 `stdin`, 5 `stdout` and
/// 6 `stderr` (with the tag of the `Input` or `Output` and its payload), 7
/// `mergeStderr()`, 8 `group()`, 9 `grace(nanoseconds)` and 10
/// `invokedAs(name)`. `stdin` is the first stage's, `stdout` the last's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_configure(
    plan_object: *mut PlanObject,
    setting: i64,
    a: *const u8,
    b: *const u8,
) -> *mut PlanObject {
    let mut stages = unsafe { (*plan_object).stages.clone() };
    let last = stages.len() - 1;
    let text = |p: *const u8| os(p as *const Str);
    match setting {
        0 => stages.iter_mut().for_each(|s| s.directory = Some(PathBuf::from(text(a)))),
        1 => stages.iter_mut().for_each(|s| s.env.push((text(a), Some(text(b))))),
        2 => stages.iter_mut().for_each(|s| s.env.push((text(a), None))),
        3 => stages.iter_mut().for_each(|s| {
            s.clear_env = true;
            s.env.clear();
        }),
        4 => stages[0].stdin = Some(unsafe { input(a as i64, b) }),
        5 => stages[last].stdout = Some(unsafe { output(a as i64, b) }),
        6 => stages[last].stderr = Some(unsafe { output(a as i64, b) }),
        7 => stages.iter_mut().for_each(|s| s.merge = true),
        8 => stages.iter_mut().for_each(|s| s.group = true),
        9 => {
            let grace = std::time::Duration::from_nanos((a as i64).max(0) as u64);
            stages.iter_mut().for_each(|s| s.grace = Some(grace));
        }
        _ => stages[0].invoked = Some(text(a)),
    }
    plan(stages)
}

/// An `Input` from its tag and payload.
unsafe fn input(tag: i64, payload: *const u8) -> Input {
    match tag {
        0 => Input::Inherit,
        1 => Input::Null,
        2 => {
            let text = payload as *const Str;
            Input::Bytes(Arc::new(unsafe { str_bytes(&text) }.to_vec()))
        }
        3 => Input::Bytes(Arc::new(unsafe { array_items::<u8>(payload as *mut ArrayHeader) }.to_vec())),
        4 => Input::File(PathBuf::from(os(payload as *const Str))),
        5 => Input::Reader(Held(retain(payload as *mut StreamObject))),
        _ => Input::Pipe,
    }
}

/// An `Output` from its tag and payload.
unsafe fn output(tag: i64, payload: *const u8) -> Output {
    match tag {
        0 => Output::Inherit,
        1 => Output::Null,
        2 => Output::Capture,
        3 => Output::File(PathBuf::from(os(payload as *const Str))),
        4 => Output::Append(PathBuf::from(os(payload as *const Str))),
        5 => Output::Writer(Held(retain(payload as *mut StreamObject))),
        6 => Output::Stdout,
        _ => Output::Pipe,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_plan_release(plan: *mut PlanObject) {
    if plan.is_null() {
        return;
    }
    unsafe {
        if (*plan).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(plan));
        LIVE_OBJECTS.gone();
    }
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// What each operation does with streams the plan leaves unset.
#[derive(Copy, Clone, PartialEq)]
enum Defaults {
    /// `run` and `start`: the program's streams.
    Inherit,
    /// `status`: none.
    Quiet,
    /// `output` and `check`: no input, and the output captured.
    Capture,
}

/// The output of a process gathered so far.
#[derive(Default)]
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<Vec<u8>>,
    combined: Vec<u8>,
}

/// Processes that started and the work that feeds and drains their
/// streams.
struct Running {
    children: Vec<Child>,
    pids: Vec<u32>,
    /// The process group of its own, `group()`'s: its first stage's id.
    group: Option<u32>,
    streams: [Option<Held>; 3],
    workers: Vec<Worker>,
    captured: Arc<Mutex<Captured>>,
    statuses: Option<Vec<(i64, bool)>>,
    detached: bool,
    /// The plan's `grace`, and whether every stage was waited for, after
    /// which a late `SIGKILL` reaches nothing.
    grace: Option<std::time::Duration>,
    reaped: Arc<Reaped>,
}

/// Whether every stage of a run was reaped, which the job that waits for
/// them does as each ends, before it reads what their pipes still hold, and
/// the tasks that wait for that: a task that ended the stages at a cancel,
/// a deadline or an alarm waits until they have ended, not for their pipes,
/// which what they started may keep open. The job alone reaps a stage, and
/// `done` and the waits change under `waiting`'s lock: a task that finds
/// the stages not reaped is in the list before the job takes it.
#[derive(Default)]
struct Reaped {
    done: AtomicBool,
    waiting: Mutex<Vec<Arc<Wait>>>,
}

impl Reaped {
    /// The job's, once it reaped every stage.
    fn set(&self) {
        let waiting = {
            let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
            self.done.store(true, Ordering::Release);
            std::mem::take(&mut *waiting)
        };
        for wait in waiting {
            if wait.claim(0) {
                notify(wait.task);
            }
        }
    }

    /// The task's, after it ended the stages: waits until the job reaped
    /// them, also in a task that is cancelled.
    fn wait(&self) {
        super::tasks::stalls::here(super::tasks::stalls::Point::Kill);
        let wait = Wait::new();
        {
            let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
            if self.done.load(Ordering::Acquire) {
                return;
            }
            waiting.push(wait.clone());
        }
        wait.block_fully();
    }
}

/// What ends the processes of a run: their ids, their group, the plan's
/// grace and whether every stage was reaped. It is fixed once they started,
/// so a `Process` keeps it beside its lock, which a wait holds.
#[derive(Clone)]
struct Stop {
    pids: Vec<u32>,
    group: Option<u32>,
    grace: Option<std::time::Duration>,
    reaped: Arc<Reaped>,
}

impl Stop {
    fn of(running: &Running) -> Stop {
        Stop { pids: running.pids.clone(), group: running.group, grace: running.grace, reaped: running.reaped.clone() }
    }

    fn now(&self) {
        stop_all(&self.pids, self.group, self.grace, &self.reaped);
    }

    /// What a cancel, a deadline or an alarm does to a wait for the run: it
    /// ends the stages and waits until they have ended, at once after a
    /// kill and after the grace at most, but not for their pipes.
    fn ended(&self) {
        self.now();
        self.reaped.wait();
    }
}

/// Where a child's output goes.
enum Sink {
    Inherit,
    Null,
    Fd(OwnedFd),
}

impl Sink {
    fn stdio(self) -> Stdio {
        match self {
            Sink::Inherit => Stdio::inherit(),
            Sink::Null => Stdio::null(),
            Sink::Fd(fd) => Stdio::from(fd),
        }
    }

    /// The same destination again, for a stream merged into it.
    fn copy(&self, standard: i32) -> std::io::Result<Sink> {
        match self {
            Sink::Inherit => {
                let fd = if standard == 1 {
                    std::io::stdout().as_fd().try_clone_to_owned()
                } else {
                    std::io::stderr().as_fd().try_clone_to_owned()
                };
                fd.map(Sink::Fd)
            }
            Sink::Null => Ok(Sink::Null),
            Sink::Fd(fd) => fd.try_clone().map(Sink::Fd),
        }
    }
}

/// What drains or feeds a child's pipe: a thread, or in a recorded run, and
/// its replay, a task of the runtime its worker drives, whose calls the
/// recording has.
enum Worker {
    Thread(super::tasks::RuntimeThread),
    Task(tokio::task::JoinHandle<()>),
}

impl Worker {
    /// Waits for it to end, or until `until`.
    fn join(self, until: Option<std::time::Instant>) {
        match self {
            Worker::Thread(handle) => {
                if let Some(until) = until {
                    while !handle.is_finished() && std::time::Instant::now() < until {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    if !handle.is_finished() {
                        return;
                    }
                }
                handle.join();
            }
            Worker::Task(handle) => {
                super::http::wait(async move {
                    match until {
                        Some(until) => drop(tokio::time::timeout_at(until.into(), handle).await),
                        None => drop(handle.await),
                    }
                });
            }
        }
    }
}

/// In a recorded run, and its replay, the runtime its worker drives, where
/// the work around children goes.
fn recorded() -> Option<&'static tokio::runtime::Runtime> {
    super::replay::on().then(super::http::runtime)
}

/// A child's pipe as the recorded run's runtime reads it.
fn receiver(runtime: &tokio::runtime::Runtime, pipe: std::io::PipeReader) -> std::io::Result<pipe::Receiver> {
    let _entered = runtime.enter();
    pipe::Receiver::from_owned_fd(pipe.into())
}

/// Copies what a child writes into the captured output and `combined`.
fn collect(mut pipe: std::io::PipeReader, captured: Arc<Mutex<Captured>>, stage: Option<usize>) -> Worker {
    let keep = move |chunk: &[u8]| {
        let mut captured = captured.lock().unwrap_or_else(|e| e.into_inner());
        match stage {
            None => captured.stdout.extend_from_slice(chunk),
            Some(stage) => captured.stderr[stage].extend_from_slice(chunk),
        }
        captured.combined.extend_from_slice(chunk);
    };
    if let Some(runtime) = recorded() {
        let receiver = receiver(runtime, pipe);
        return Worker::Task(runtime.spawn(async move {
            let Ok(mut receiver) = receiver else { return };
            let mut buffer = vec![0; 64 * 1024];
            while let Ok(count) = receiver.read(&mut buffer).await
                && count > 0
            {
                keep(&buffer[..count]);
            }
        }));
    }
    Worker::Thread(super::tasks::spawn_thread(None, move || {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            keep(&buffer[..count]);
        }
    }))
}

/// Copies what a child writes into a stream of `std.io`.
fn forward(mut pipe: std::io::PipeReader, target: Held) -> Worker {
    if let Some(runtime) = recorded() {
        let receiver = receiver(runtime, pipe);
        return Worker::Task(runtime.spawn(async move {
            let target = Arc::new(target);
            if let Ok(mut receiver) = receiver {
                let mut buffer = vec![0; 64 * 1024];
                while let Ok(count) = receiver.read(&mut buffer).await
                    && count > 0
                {
                    let (target, chunk) = (target.clone(), buffer[..count].to_vec());
                    let written = super::http::blocking(true, move || run(target.0, move |s| s.write(&chunk))).await;
                    if !matches!(written, Ok(Ok(()))) {
                        break;
                    }
                }
            }
            let _ = super::http::blocking(true, move || run(target.0, |s| s.flush())).await;
        }));
    }
    Worker::Thread(super::tasks::spawn_thread(None, move || {
        let target = target;
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let chunk = buffer[..count].to_vec();
            if run(target.0, move |s| s.write(&chunk)).is_err() {
                break;
            }
        }
        let _ = run(target.0, |s| s.flush());
    }))
}

/// Feeds a child's input from bytes or a stream of `std.io`.
fn feed(mut pipe: std::io::PipeWriter, input: Input) -> Worker {
    if let Some(runtime) = recorded() {
        let sender = {
            let _entered = runtime.enter();
            pipe::Sender::from_owned_fd(pipe.into())
        };
        return Worker::Task(runtime.spawn(async move {
            let Ok(mut sender) = sender else { return };
            match input {
                Input::Bytes(bytes) => drop(sender.write_all(&bytes).await),
                Input::Reader(source) => {
                    let source = Arc::new(source);
                    loop {
                        let reading = source.clone();
                        match super::http::blocking(true, move || run(reading.0, |s| s.read(64 * 1024))).await {
                            Ok(Ok(Some(chunk))) if sender.write_all(&chunk).await.is_ok() => {}
                            _ => break,
                        }
                    }
                }
                _ => {}
            }
        }));
    }
    Worker::Thread(super::tasks::spawn_thread(None, move || match input {
        Input::Bytes(bytes) => {
            let _ = pipe.write_all(&bytes);
        }
        Input::Reader(source) => {
            let source = source;
            while let Ok(Some(chunk)) = run(source.0, |s| s.read(64 * 1024)) {
                if pipe.write_all(&chunk).is_err() {
                    break;
                }
            }
        }
        _ => {}
    }))
}

/// Where output set to `Inherit` goes: the console's stream when the
/// program replaced it. `standard` is 1 for stdout and 2 for stderr; the
/// console numbers its output and error 0 and 1.
fn inherited(standard: usize) -> Output {
    match replaced_console(standard - 1) {
        Some(stream) => Output::Writer(Held(stream)),
        None => Output::Inherit,
    }
}

/// The sink for an output, starting the work that drains it.
fn sink(out: Output, stage: Option<usize>, running: &mut Running, standard: usize) -> std::io::Result<Sink> {
    let out = if matches!(out, Output::Inherit) { inherited(standard) } else { out };
    Ok(match out {
        Output::Inherit => Sink::Inherit,
        Output::Null => Sink::Null,
        Output::File(path) => Sink::Fd(std::fs::File::create(path)?.into()),
        Output::Append(path) => Sink::Fd(std::fs::OpenOptions::new().append(true).create(true).open(path)?.into()),
        Output::Capture => {
            let (reader, writer) = std::io::pipe()?;
            running.workers.push(collect(reader, running.captured.clone(), stage));
            Sink::Fd(writer.into())
        }
        Output::Writer(target) => {
            let (reader, writer) = std::io::pipe()?;
            running.workers.push(forward(reader, target));
            Sink::Fd(writer.into())
        }
        Output::Pipe => {
            let (reader, writer) = std::io::pipe()?;
            running.streams[standard] = Some(Held(reading_end(reader)?));
            Sink::Fd(writer.into())
        }
        Output::Stdout => unreachable!("merged by the caller"),
    })
}

/// Starts every stage of a plan, connected by pipes.
fn spawn(plan: &PlanObject, defaults: Defaults) -> std::io::Result<Running> {
    let stages = &plan.stages;
    let mut running = Running {
        children: Vec::new(),
        pids: Vec::new(),
        group: None,
        streams: [None, None, None],
        workers: Vec::new(),
        captured: Arc::new(Mutex::new(Captured { stderr: vec![Vec::new(); stages.len()], ..Captured::default() })),
        statuses: None,
        detached: false,
        grace: stages.iter().find_map(|stage| stage.grace),
        reaped: Arc::default(),
    };
    // Output written so far reaches the terminal before the children's.
    super::flush_stdout();
    let environment = super::env::inherited();
    let mut previous: Option<std::io::PipeReader> = None;
    let started = (|| {
        for (index, stage) in stages.iter().enumerate() {
            let last = index + 1 == stages.len();
            let path = child_path(stage, environment.as_deref().map(Vec::as_slice));
            let found = located(&stage.program, path.as_deref(), stage.directory.as_deref())?;
            let mut command =
                std::process::Command::new(found.as_deref().map_or(stage.program.as_os_str(), Path::as_os_str));
            command.args(&stage.arguments);
            // Its `argv[0]`, which `ps` shows, stays the name the plan gave.
            command.arg0(stage.invoked.as_ref().unwrap_or(&stage.program));
            // The first stage leads the group, and the others join it.
            if stage.group {
                command.process_group(running.group.map_or(0, |group| group as i32));
            }
            if stage.clear_env {
                command.env_clear();
            } else if let Some(environment) = &environment {
                command.env_clear();
                command.envs(environment.iter().map(|(k, v)| (k, v)));
            }
            for (name, value) in &stage.env {
                match value {
                    Some(value) => command.env(name, value),
                    None => command.env_remove(name),
                };
            }
            if let Some(directory) = &stage.directory {
                command.current_dir(directory);
            }
            // Input: the previous stage's output, or the first stage's.
            let stdin = match previous.take() {
                Some(pipe) => Stdio::from(pipe),
                None => {
                    let default = if defaults == Defaults::Inherit { Input::Inherit } else { Input::Null };
                    match stage.stdin.clone().unwrap_or(default) {
                        Input::Inherit => match replaced_console(2) {
                            Some(stream) => {
                                let (reader, writer) = std::io::pipe()?;
                                running.workers.push(feed(writer, Input::Reader(Held(stream))));
                                Stdio::from(reader)
                            }
                            None => Stdio::inherit(),
                        },
                        Input::Null => Stdio::null(),
                        Input::File(path) => Stdio::from(std::fs::File::open(path)?),
                        Input::Pipe => {
                            let (reader, writer) = std::io::pipe()?;
                            running.streams[0] = Some(Held(writing_end(writer)?));
                            Stdio::from(reader)
                        }
                        fed => {
                            let (reader, writer) = std::io::pipe()?;
                            running.workers.push(feed(writer, fed));
                            Stdio::from(reader)
                        }
                    }
                }
            };
            command.stdin(stdin);
            // Output: the next stage's input, or the last stage's.
            let stdout = if last {
                let default = match defaults {
                    Defaults::Inherit => Output::Inherit,
                    Defaults::Quiet => Output::Null,
                    Defaults::Capture => Output::Capture,
                };
                sink(stage.stdout.clone().unwrap_or(default), None, &mut running, 1)?
            } else {
                let (reader, writer) = std::io::pipe()?;
                previous = Some(reader);
                Sink::Fd(writer.into())
            };
            let error = stage.stderr.clone().unwrap_or(match defaults {
                Defaults::Inherit => Output::Inherit,
                Defaults::Quiet => Output::Null,
                Defaults::Capture => Output::Capture,
            });
            let stderr = if stage.merge || matches!(error, Output::Stdout) {
                stdout.copy(1)?
            } else {
                sink(error, Some(index), &mut running, 2)?
            };
            command.stdout(stdout.stdio()).stderr(stderr.stdio());
            let followed = super::debugger::following();
            if followed {
                command.env("TONDO_DEBUG", "1");
            }
            if let Some(children) = super::replay::children() {
                command.env("TONDO_RT_RECORD_CHILDREN", children);
            }
            let child = command.spawn()?;
            let program = stage.program.to_string_lossy();
            if followed {
                super::debugger::child_started(child.id(), &program, None);
            } else if super::debugger::follows_children()
                && let Some(recording) = super::replay::recorded_child(child.id())
            {
                super::debugger::child_started(child.id(), &program, Some(&recording));
            }
            super::diagnostics::PROCESSES_STARTED.add(1);
            running.pids.push(child.id());
            if stage.group && running.group.is_none() {
                running.group = Some(child.id());
            }
            running.children.push(child);
            // `command` drops here, closing this side's copies of the pipes.
        }
        Ok(())
    })();
    if let Err(error) = started {
        kill_all(&running.pids, running.group);
        let _ = finish(&mut running);
        return Err(error);
    }
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).extend(running.pids.iter().map(|&pid| (pid, running.group)));
    Ok(running)
}

/// The `PATH` a stage's program is looked up on: its environment's.
fn child_path(stage: &Stage, environment: Option<&[(OsString, OsString)]>) -> Option<OsString> {
    for (name, value) in stage.env.iter().rev() {
        if name == "PATH" {
            return value.clone();
        }
    }
    if stage.clear_env {
        return None;
    }
    match environment {
        Some(environment) => environment.iter().find(|(name, _)| name == "PATH").map(|(_, value)| value.clone()),
        None => super::env::path_var(),
    }
}

/// What `execvp` looks on when the child's environment has no `PATH`: the
/// C library's own.
#[cfg(target_os = "macos")]
const DEFAULT_PATH: &str = "/usr/bin:/bin";
#[cfg(all(target_os = "linux", target_env = "musl"))]
const DEFAULT_PATH: &str = "/usr/local/bin:/bin:/usr/bin";
#[cfg(all(target_os = "linux", not(target_env = "musl")))]
const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// The file a child runs for `program`, looked up as `execvp` would in the
/// child: a name with a `/` is not looked up; an entry of `path` that is
/// empty, or not absolute, is from the child's `directory`; an entry where
/// nothing it may run is found is passed over; with no `PATH`, the C
/// library's own. Nothing to run is `NotFound`, and only files it may not
/// run `PermissionDenied`, as `execvp` says.
///
/// Given its path, std starts a child with `posix_spawn`; by a name looked
/// up on a `PATH` set for the child, as every start sets its environment,
/// it forks and execs, copying the parent's map of memory, which with the
/// regions' checks took a third of a second on macOS where `posix_spawn`
/// takes a millisecond. `None` starts it by its name as before: one with a
/// `/`, or a file that only `execvp` runs, a script with no `#!` that it
/// hands to the shell.
fn located(program: &OsStr, path: Option<&OsStr>, directory: Option<&Path>) -> std::io::Result<Option<PathBuf>> {
    let program = program.as_bytes();
    if program.is_empty() || program.contains(&b'/') || program.contains(&0) {
        return Ok(None);
    }
    let path = path.map_or(DEFAULT_PATH.as_bytes(), OsStr::as_bytes);
    let mut base: Option<PathBuf> = None;
    let mut buf = Vec::with_capacity(128);
    let mut denied = false;
    for entry in path.split(|&byte| byte == b':') {
        if entry.contains(&0) {
            continue;
        }
        buf.clear();
        if entry.first() == Some(&b'/') {
            buf.extend_from_slice(entry);
        } else {
            let root = match &base {
                Some(root) => root,
                None => {
                    let here = std::env::current_dir()?;
                    let resolved = directory.map_or(here.clone(), |dir| here.join(dir));
                    base.insert(resolved)
                }
            };
            buf.extend_from_slice(root.as_os_str().as_bytes());
            if !entry.is_empty() {
                if buf.last() != Some(&b'/') {
                    buf.push(b'/');
                }
                buf.extend_from_slice(entry);
            }
        }
        if buf.last() != Some(&b'/') {
            buf.push(b'/');
        }
        buf.extend_from_slice(program);
        buf.push(0);
        let cpath = buf.as_ptr().cast();
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::stat(cpath, &mut st) } == 0 {
            if (st.st_mode & libc::S_IFMT) == libc::S_IFREG && unsafe { libc::access(cpath, libc::X_OK) } == 0 {
                return Ok(started_by_path_cstr(&buf));
            }
            denied = true;
        } else if matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EACCES | libc::EPERM)) {
            denied = true;
        }
    }
    Err(if denied { ErrorKind::PermissionDenied } else { ErrorKind::NotFound }.into())
}

/// The file when the system runs it by itself, as its first bytes say: a
/// `#!` line, or an executable of ELF or Mach-O. Another goes to `execvp`.
fn started_by_path_cstr(nul_path: &[u8]) -> Option<PathBuf> {
    let mut head = [0u8; 4];
    let fd = unsafe { libc::open(nul_path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    let read = if fd >= 0 {
        let mut got = 0usize;
        while got < head.len() {
            let n = unsafe { libc::read(fd, head[got..].as_mut_ptr().cast(), head.len() - got) };
            if n <= 0 {
                if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                break;
            }
            got += n as usize;
        }
        unsafe { libc::close(fd) };
        got
    } else {
        0
    };
    let shebang = read >= 2 && head.starts_with(b"#!");
    let binary = read == 4
        && (head == *b"\x7fELF"
            || matches!(
                u32::from_be_bytes(head),
                0xfeed_face | 0xfeed_facf | 0xcefa_edfe | 0xcffa_edfe | 0xcafe_babe | 0xbeba_feca
            ));
    (shebang || binary).then(|| PathBuf::from(OsStr::from_bytes(&nul_path[..nul_path.len() - 1])))
}

/// The children of processes that started and nobody waited for, which a
/// panic cancels.
static LIVE: Mutex<Vec<(u32, Option<u32>)>> = Mutex::new(Vec::new());

/// How many processes the program started are running.
pub(crate) fn running() -> usize {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// Ends the processes: at once, or with `grace`, `SIGTERM` first and then
/// `SIGKILL` when it passes, as `docker stop` does. The group gets them
/// whenever there is one, and the stages only while they are not reaped: a
/// reaped stage's id may name another process by then.
fn stop_all(pids: &[u32], group: Option<u32>, grace: Option<std::time::Duration>, reaped: &Arc<Reaped>) {
    let pids = if reaped.done.load(Ordering::Acquire) { &[] } else { pids };
    let Some(grace) = grace else { return kill_all(pids, group) };
    match group {
        Some(group) => unsafe {
            kill(-(group as i32), SIGTERM);
        },
        None => pids.iter().for_each(|&pid| unsafe {
            kill(pid as i32, SIGTERM);
        }),
    }
    let (pids, reaped) = (pids.to_vec(), reaped.clone());
    // Its id names no other group while a member lives.
    let late = move || {
        let pids: &[u32] = if reaped.done.load(Ordering::Acquire) { &[] } else { &pids };
        kill_all(pids, group);
    };
    match recorded() {
        Some(runtime) => drop(runtime.spawn(async move {
            tokio::time::sleep(grace).await;
            late();
        })),
        None => drop(super::tasks::spawn_thread(None, move || {
            std::thread::sleep(grace);
            late();
        })),
    }
}

/// Ends the processes, and every process of their group when they have
/// one of their own: what they started too.
fn kill_all(pids: &[u32], group: Option<u32>) {
    if let Some(group) = group {
        unsafe { kill(-(group as i32), SIGKILL) };
    }
    for &pid in pids {
        unsafe { kill(pid as i32, SIGKILL) };
    }
}

/// Cancels the processes still running when a panic ends the program.
pub(crate) fn cancel_live() {
    let live = std::mem::take(&mut *LIVE.lock().unwrap_or_else(|e| e.into_inner()));
    for (pid, group) in live {
        kill_all(&[pid], group);
    }
}

/// Waits for every stage and the work around them; the statuses as
/// `(code, success)`.
fn finish(running: &mut Running) -> Vec<(i64, bool)> {
    if let Some(statuses) = &running.statuses {
        return statuses.clone();
    }
    // The program's side of a piped input closes, so the child sees its end.
    running.streams[0] = None;
    let ends = Stop::of(running);
    let stop = || ends.now();
    let statuses: Vec<(i64, bool)> = running
        .children
        .iter_mut()
        .map(|child| match exited(child, &stop) {
            Ok(status) => match (status.code(), status.signal()) {
                (Some(code), _) => (code as i64, code == 0),
                (None, Some(signal)) => (128 + signal as i64, false),
                _ => (1, false),
            },
            Err(_) => (1, false),
        })
        .collect();
    running.reaped.set();
    // With `grace`, descendants that keep the captured output open get
    // that long to finish it.
    let until = running.grace.map(|grace| std::time::Instant::now() + grace);
    for worker in running.workers.drain(..) {
        worker.join(until);
    }
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|(pid, _)| !running.pids.contains(pid));
    running.statuses = Some(statuses.clone());
    statuses
}

/// Waits for a child to end. In a recorded run, the worker drives its
/// runtime meanwhile, which polls the child's pidfd: what drains the
/// child's pipes runs there. A task cancelled meanwhile `stop`s the child,
/// as `host_cancellable` does elsewhere, and the worker goes on driving
/// what ends it after its grace.
fn exited(child: &mut Child, stop: &dyn Fn()) -> std::io::Result<std::process::ExitStatus> {
    let (Some(runtime), Some(fd)) = (recorded(), pidfd(child.id())) else { return child.wait() };
    let waited = super::http::wait(async move {
        let interest = tokio::io::Interest::READABLE;
        // The pidfd, open until the wait ends.
        if let Ok(fd) = unsafe { tokio::io::unix::AsyncFd::register_with_interest(fd, interest) } {
            let _ = fd.readable().await;
        }
    });
    if waited.is_some() {
        return child.wait();
    }
    stop();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        super::http::drive(runtime, false, None);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// A descriptor that becomes readable when the child `pid` ends, where a
/// run is recorded: a pidfd, or a kqueue that has only that event; none
/// when the child ended already.
fn pidfd(pid: u32) -> Option<OwnedFd> {
    #[cfg(target_os = "linux")]
    {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        (fd >= 0).then(|| unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd as i32) })
    }
    #[cfg(target_os = "macos")]
    {
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return None;
        }
        let queue = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        let mut change: libc::kevent = unsafe { std::mem::zeroed() };
        change.ident = pid as usize;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD;
        change.fflags = libc::NOTE_EXIT;
        let added = unsafe { libc::kevent(fd, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
        (added == 0).then_some(queue)
    }
}

/// The status of the whole plan: success when every stage succeeds, and
/// the code of the last stage that fails, or of the last. A stage ended by
/// a closed pipe before a later stage succeeded did not fail.
fn overall(statuses: &[(i64, bool)]) -> (i64, bool) {
    let last = statuses.len() - 1;
    let failed = |index: usize, &(code, success): &(i64, bool)| {
        !success && !(index < last && code == 128 + SIGPIPE as i64 && statuses[last].1)
    };
    match statuses.iter().enumerate().rev().find(|(i, s)| failed(*i, s)) {
        Some((_, &(code, _))) => (code, false),
        None => (statuses[last].0, true),
    }
}

/// `ProcessError` statuses.
const NOT_FOUND: i64 = 1;
const PERMISSION_DENIED: i64 = 2;
const INVALID_DATA: i64 = 3;
const FAILED: i64 = 4;
const HOST: i64 = 5;

fn error_status(error: &std::io::Error) -> i64 {
    match error.kind() {
        ErrorKind::NotFound => NOT_FOUND,
        ErrorKind::PermissionDenied => PERMISSION_DENIED,
        _ => HOST,
    }
}

/// The layout of an `ExitStatus` element: its size and where `success` is.
#[derive(Copy, Clone)]
struct StatusLayout {
    size: usize,
    success: usize,
}

/// What a run that ended gave: the status of each stage, and what was
/// captured, which no thread writes any more.
type Ended = (Vec<(i64, bool)>, Option<Arc<Mutex<Captured>>>);

/// What a run gave, on the thread of the host that waited for it to end:
/// nothing captured when `failed_only` and every stage succeeded.
fn ended(running: &mut Running, failed_only: bool) -> Ended {
    let statuses = finish(running);
    let captured = (!failed_only || !overall(&statuses).1).then(|| running.captured.clone());
    (statuses, captured)
}

/// Writes a `ProcessOutput` as four words, then the overall status as two:
/// stdout, stderr, combined, statuses, code and success; nothing without
/// `captured`. The task makes its arrays, not the thread that waited.
unsafe fn write_output(
    statuses: &[(i64, bool)],
    captured: Option<Arc<Mutex<Captured>>>,
    layout: StatusLayout,
    out: *mut i64,
) {
    let Some(captured) = captured else { return };
    let captured = captured.lock().unwrap_or_else(|e| e.into_inner());
    let (code, success) = overall(statuses);
    let array = tondo_rt_array_new(layout.size, statuses.len());
    for (index, &(code, success)) in statuses.iter().enumerate() {
        unsafe {
            let at = element(array, layout.size, index);
            *(at as *mut i64) = code;
            *at.add(layout.success) = success as u8;
        }
    }
    let words = [
        byte_array(&captured.stdout) as i64,
        byte_array(&captured.stderr.concat()) as i64,
        byte_array(&captured.combined) as i64,
        array as i64,
        code,
        success as i64,
    ];
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr(), out, words.len()) };
}

fn plan_ref<'a>(plan: *mut PlanObject) -> &'a PlanObject {
    unsafe { &*plan }
}

/// `run()` (`quiet` 0) and `status()` (1): the overall status into `out`
/// as code and success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_run(plan: *mut PlanObject, quiet: u8, out: *mut i64) -> i64 {
    let defaults = if quiet != 0 { Defaults::Quiet } else { Defaults::Inherit };
    let mut running = match spawn(plan_ref(plan), defaults) {
        Ok(running) => running,
        Err(error) => return error_status(&error),
    };
    let stop = Stop::of(&running);
    let Some(statuses) = host_cancellable(move || finish(&mut running), || stop.ended(), false) else {
        unsafe { std::ptr::write_bytes(out, 0, 2) };
        super::panics::start_noted();
        return 0;
    };
    let (code, success) = overall(&statuses);
    unsafe { std::ptr::copy_nonoverlapping([code, success as i64].as_ptr(), out, 2) };
    0
}

/// `output()` (`check` 0) and `check()` (1): a `ProcessOutput` into `out`
/// (see `write_output`); `check` fails with `Failed` when a stage did. And
/// `run()` (2): the program's streams, and nothing into `out` unless it
/// fails so, with the status of each stage and nothing captured. `text()`
/// (3) and `lines()` (4) are `check()`, then its standard output as text
/// without the new lines at its end, or by lines, into `out`: `InvalidData`
/// when it is not UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_output(
    plan: *mut PlanObject,
    check: u8,
    size: i64,
    success_offset: i64,
    out: *mut i64,
    region: *mut crate::regions::Region,
) -> i64 {
    let layout = StatusLayout { size: size as usize, success: success_offset as usize };
    let run = check == 2;
    let mut running = match spawn(plan_ref(plan), if run { Defaults::Inherit } else { Defaults::Capture }) {
        Ok(running) => running,
        Err(error) => return error_status(&error),
    };
    let stop = Stop::of(&running);
    unsafe { std::ptr::write_bytes(out, 0, 6) };
    let Some((statuses, captured)) = host_cancellable(move || ended(&mut running, run), || stop.ended(), false) else {
        super::panics::start_noted();
        return 0;
    };
    let success = overall(&statuses).1;
    if check < 3 || !success {
        unsafe { write_output(&statuses, captured, layout, out) };
        return if check != 0 && !success { FAILED } else { 0 };
    }
    // The text of standard output, and no arrays of the output.
    let Some(captured) = captured else { return INVALID_DATA };
    let captured = captured.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(text) = simdutf8::basic::from_utf8(&captured.stdout) else { return INVALID_DATA };
    let text = text.trim_end_matches(['\n', '\r']);
    unsafe {
        *out = if check == 3 {
            crate::strings::str_in(region, text.as_bytes()) as i64
        } else {
            crate::arrays::texts_array_in(region, || text.lines().map(str::as_bytes)) as i64
        };
    }
    0
}

/// A started process. The job of each wait for it holds a count of it.
#[repr(C)]
pub struct ProcessObject {
    rc: AtomicUsize,
    stop: Stop,
    running: Mutex<Running>,
}

struct Process(*mut ProcessObject);

unsafe impl Send for Process {}

impl Process {
    fn running(&self) -> MutexGuard<'_, Running> {
        unsafe { (*self.0).running.lock().unwrap_or_else(|e| e.into_inner()) }
    }
}

/// `start()`: the process into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_start(plan: *mut PlanObject, out: *mut *mut ProcessObject) -> i64 {
    crate::status(
        spawn(plan_ref(plan), Defaults::Inherit),
        |error| error_status(&error),
        |running| {
            LIVE_OBJECTS.made();
            let process =
                ProcessObject { rc: AtomicUsize::new(1), stop: Stop::of(&running), running: Mutex::new(running) };
            unsafe { *out = Box::into_raw(Box::new(process)) };
        },
    )
}

/// `stdin()`, `stdout()` and `stderr()` (0, 1, 2): the piped stream, or a
/// closed one when the plan did not pipe it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_stream(process: *mut ProcessObject, which: i64) -> *mut StreamObject {
    let running = Process(process);
    let running = running.running();
    match &running.streams[which as usize] {
        Some(held) => retain(held.0),
        None => {
            let closed = unsafe { super::io::tondo_rt_io_discard(std::ptr::null_mut()) };
            super::io::run(closed, |s| {
                let _ = s.close();
            });
            closed
        }
    }
}

/// `id()`: the system's id of the last stage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_id(process: *mut ProcessObject) -> i64 {
    unsafe { (*process).stop.pids.last() }.map_or(0, |&pid| pid as i64)
}

/// `Process.signal`: sends the signal of `process.Signal`'s variant `tag`
/// to each stage of the process, and to its whole group when it has one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_signal(process: *mut ProcessObject, tag: i64) {
    let Some(&signal) = super::signals::SIGNALS.get(tag as usize) else { return };
    let stop = unsafe { &(*process).stop };
    if let Some(group) = stop.group {
        unsafe { libc::kill(-(group as i32), signal) };
    }
    for &pid in &stop.pids {
        unsafe { libc::kill(pid as i32, signal) };
    }
}

/// `process.which(name)`: the file a shell runs for `name`, with the
/// program's `PATH`, in a directory whose path is a text, as the path it
/// gives must be; empty for none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_which(name: *const Str, region: *mut crate::regions::Region) -> *mut Str {
    let name = os(name);
    let runnable = |path: &std::path::Path| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|found| found.is_file() && found.permissions().mode() & 0o111 != 0)
    };
    let found = if name.as_bytes().contains(&b'/') {
        Some(PathBuf::from(&name)).filter(|path| runnable(path))
    } else {
        let path = super::env::path_var();
        path.unwrap_or_default().as_bytes().split(|&b| b == b':').find_map(|directory| {
            let directory = if directory.is_empty() { OsStr::new(".") } else { OsStr::from_bytes(directory) };
            Some(std::path::Path::new(directory).join(&name)).filter(|path| path.to_str().is_some() && runnable(path))
        })
    };
    crate::strings::str_in(region, found.map_or(Vec::new(), |path| path.into_os_string().into_vec()).as_slice())
}

/// `process.signal(pid, signal)`, to a process the program did not start:
/// 0, or 1 when there is none and 2 when the program may not signal it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_process_kill(pid: i64, tag: i64) -> i64 {
    let Some(&signal) = super::signals::SIGNALS.get(tag as usize) else { return 1 };
    let Ok(pid) = i32::try_from(pid) else { return 1 };
    if pid <= 0 || unsafe { libc::kill(pid, signal) } == 0 {
        // To the program itself, the signal arrives while it goes on: a
        // window of `--schedules` (testing 8.10).
        if pid == std::process::id() as i32 {
            crate::tasks::window();
        }
        return (pid <= 0) as i64;
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EPERM) => 2,
        _ => 1,
    }
}

/// Waits for the process, with `cancel` killing it first; a cancelled task
/// ends it too and goes on once its stages have ended, and `give_up` does
/// so at once. The job holds a count of the process until it ends, so a
/// cancelled task may let go of its own.
fn settle<R: Send + 'static>(
    process: *mut ProcessObject,
    cancel: bool,
    give_up: bool,
    then: impl FnOnce(&mut Running) -> R + Send + 'static,
) -> Option<R> {
    let stop = unsafe { (*process).stop.clone() };
    if cancel {
        stop.now();
    }
    unsafe { (*process).rc.fetch_add(1, Ordering::Relaxed) };
    let held = Process(process);
    host_cancellable(
        move || {
            let value = then(&mut held.running());
            unsafe { let_go(held.0) };
            value
        },
        move || stop.ended(),
        give_up,
    )
}

/// `wait()` and `cancel()` (`cancel` 1): the overall status into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_wait(process: *mut ProcessObject, cancel: u8, out: *mut i64) -> i64 {
    let Some(statuses) = settle(process, cancel != 0, false, finish) else {
        unsafe { std::ptr::write_bytes(out, 0, 2) };
        super::panics::start_noted();
        return 0;
    };
    let (code, success) = overall(&statuses);
    unsafe { std::ptr::copy_nonoverlapping([code, success as i64].as_ptr(), out, 2) };
    0
}

/// `Process.output()` and `check()`, like `tondo_rt_process_output`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_collect(
    process: *mut ProcessObject,
    check: u8,
    size: i64,
    success_offset: i64,
    out: *mut i64,
) -> i64 {
    let layout = StatusLayout { size: size as usize, success: success_offset as usize };
    unsafe { std::ptr::write_bytes(out, 0, 6) };
    let Some((statuses, captured)) = settle(process, false, false, |running| ended(running, false)) else {
        super::panics::start_noted();
        return 0;
    };
    unsafe { write_output(&statuses, captured, layout, out) };
    if check != 0 && !overall(&statuses).1 { FAILED } else { 0 }
}

/// `detach()`: the processes run on; a thread of their own reaps them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_detach(process: *mut ProcessObject) {
    let process = Process(process);
    let mut running = process.running();
    if running.statuses.is_some() {
        return;
    }
    running.detached = true;
    let mut gone = Running {
        children: std::mem::take(&mut running.children),
        pids: running.pids.clone(),
        group: running.group,
        streams: [running.streams[0].take(), running.streams[1].take(), running.streams[2].take()],
        workers: std::mem::take(&mut running.workers),
        captured: running.captured.clone(),
        statuses: None,
        detached: true,
        grace: running.grace,
        reaped: running.reaped.clone(),
    };
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|(pid, _)| !gone.pids.contains(pid));
    // The program's side of a piped input closes first.
    drop(std::mem::take(&mut gone.streams));
    match recorded() {
        // A recorded run waits for each child as it ends, on its worker.
        Some(runtime) => {
            for child in std::mem::take(&mut gone.children) {
                let fd = pidfd(child.id());
                let mut child = child;
                let _entered = runtime.enter();
                let ready = fd.map(|fd| unsafe {
                    tokio::io::unix::AsyncFd::register_with_interest(fd, tokio::io::Interest::READABLE)
                });
                drop(runtime.spawn(async move {
                    if let Some(Ok(ready)) = &ready {
                        let _ = ready.readable().await;
                    }
                    let _ = child.wait();
                }));
            }
        }
        None => drop(super::tasks::spawn_thread(None, move || {
            for child in &mut gone.children {
                let _ = child.wait();
            }
            for worker in gone.workers.drain(..) {
                worker.join(None);
            }
        })),
    }
}

/// Releases a `Process`: the last reference closes its input and waits for
/// it, unless it was detached. A release gives nothing back and starts no
/// unwinding. Dropped by a panic's unwinding, or in a cancelled task, it
/// ends the stages and waits for them, not for their pipes, and the job
/// lets go of the process (19.2 of the library).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_release(process: *mut ProcessObject) {
    if process.is_null() {
        return;
    }
    unsafe {
        if (*process).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        if Process(process).running().detached {
            drop(Box::from_raw(process));
            LIVE_OBJECTS.gone();
            return;
        }
        // No count is left, and no job runs: `settle` gives its job the
        // only one, whose end lets go of the process.
        let _ = settle(process, false, super::panics::cleaning_up(), finish);
    }
}

/// Lets go of a count of a `Process` that a job held: the last waits for
/// it, unless it was detached, and frees it.
unsafe fn let_go(process: *mut ProcessObject) {
    unsafe {
        if (*process).rc.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        std::sync::atomic::fence(Ordering::Acquire);
        let mut running = (*process).running.lock().unwrap_or_else(|e| e.into_inner());
        if !running.detached {
            finish(&mut running);
        }
        drop(running);
        drop(Box::from_raw(process));
        LIVE_OBJECTS.gone();
    }
}

/// `ProcessOutput.text()`: the stdout as UTF-8 into `out`, or `InvalidData`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_text(
    bytes: *mut ArrayHeader,
    out: *mut *mut Str,
    region: *mut crate::regions::Region,
) -> i64 {
    let text = simdutf8::basic::from_utf8(unsafe { array_items::<u8>(bytes) });
    crate::status(text, |_| INVALID_DATA, |text| unsafe { *out = crate::strings::str_in(region, text.as_bytes()) })
}

/// `process.exit(code)`: delivers the console's output and ends the program.
/// `exec()`: the program becomes the command's, with its environment,
/// directory and redirections to files; it returns only when it cannot,
/// with a `ProcessError` status.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_exec(plan: *mut PlanObject, _out: *mut i64) -> i64 {
    let Some(stage) = plan_ref(plan).stages.first() else { return HOST };
    let mut command = std::process::Command::new(&stage.program);
    command.args(&stage.arguments);
    if let Some(invoked) = &stage.invoked {
        command.arg0(invoked);
    }
    if stage.group {
        command.process_group(0);
    }
    if stage.clear_env {
        command.env_clear();
    } else if let Some(environment) = super::env::inherited() {
        command.env_clear();
        command.envs(environment.iter().map(|(k, v)| (k, v)));
    }
    for (name, value) in &stage.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    if let Some(directory) = &stage.directory {
        command.current_dir(directory);
    }
    // Only what outlives this program: the terminal, nothing, or a file.
    let file = |path: &PathBuf, append: bool| {
        std::fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(path)
    };
    let opened = (|| {
        match &stage.stdin {
            None | Some(Input::Inherit) => {}
            Some(Input::Null) => _ = command.stdin(Stdio::null()),
            Some(Input::File(path)) => _ = command.stdin(Stdio::from(std::fs::File::open(path)?)),
            Some(_) => return Err(std::io::Error::from(ErrorKind::InvalidData)),
        }
        for (output, standard) in [(&stage.stdout, 1), (&stage.stderr, 2)] {
            let stdio = match output {
                None | Some(Output::Inherit) => continue,
                Some(Output::Null) => Stdio::null(),
                Some(Output::File(path)) => Stdio::from(file(path, false)?),
                Some(Output::Append(path)) => Stdio::from(file(path, true)?),
                Some(_) => return Err(std::io::Error::from(ErrorKind::InvalidData)),
            };
            if standard == 1 {
                command.stdout(stdio);
            } else {
                command.stderr(stdio);
            }
        }
        Ok(())
    })();
    if let Err(error) = opened {
        return error_status(&error);
    }
    let _ = super::finish_stdout();
    // The program that takes the process's place finds the terminal as this
    // one did; when it cannot start, this one goes on with it taken again.
    error_status(&super::terminal::around(|| command.exec()))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_process_exit(code: i64) -> ! {
    super::terminal::give_back();
    let _ = super::finish_stdout();
    std::process::exit(code as i32)
}

/// A `ProcessError` in words, with the code of `Failed`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_sb_process_error(b: *mut super::Builder, tag: i64, code: i64) {
    let text = match tag {
        0 => "not found".to_string(),
        1 => "permission denied".to_string(),
        2 => "invalid data".to_string(),
        3 => format!("exited with code {code}"),
        // `InvalidCommand`, whose text is what `code` points to.
        5 => {
            let text = code as *const Str;
            String::from_utf8_lossy(unsafe { str_bytes(&text) }).into_owned()
        }
        _ => "host".to_string(),
    };
    unsafe { (*b).extend(text.as_bytes()) }
}

// ---------------------------------------------------------------------------
// The program's ends of a started process's pipes
// ---------------------------------------------------------------------------

/// The stream of the end of an output pipe the program reads: on the
/// workers' poller, where a read that would wait leaves its task, so that a
/// deadline or a cancel ends the wait and what the child writes stays in the
/// pipe for the next read; on a thread of the pool where nothing polls.
fn reading_end(reader: std::io::PipeReader) -> std::io::Result<*mut StreamObject> {
    if super::poll::started().is_none() {
        return Ok(stream(Box::new(reader), Mode::Long, false));
    }
    // Ours alone: the child's end is another open file description, which
    // stays as it was.
    let pipe = mio::unix::pipe::Receiver::from(OwnedFd::from(reader));
    pipe.set_nonblocking(true)?;
    let pipe = Registered::with(pipe, mio::Interest::READABLE)?;
    Ok(stream(Box::new(PipeIn { pipe, waiting: false, gave_up: None }), Mode::Ready, false))
}

/// The stream of the end of an input pipe the program writes, as
/// `reading_end`; see `PipeOut`.
fn writing_end(writer: std::io::PipeWriter) -> std::io::Result<*mut StreamObject> {
    if super::poll::started().is_none() {
        return Ok(stream(Box::new(writer), Mode::Long, false));
    }
    let pipe = mio::unix::pipe::Sender::from(OwnedFd::from(writer));
    pipe.set_nonblocking(true)?;
    quiet(&pipe)?;
    let pipe = Registered::with(pipe, mio::Interest::WRITABLE)?;
    let outgoing = Arc::new(Outgoing { pipe, unsent: Mutex::new(Unsent::default()) });
    let weak = Arc::downgrade(&outgoing);
    outgoing.pipe.on_writable(Arc::new(move || {
        if let Some(outgoing) = weak.upgrade() {
            outgoing.writable();
        }
    }));
    Ok(stream(Box::new(PipeOut { outgoing, waiting: false, gave_up: None }), Mode::Ready, false))
}

/// The end of a child's output pipe that the program reads.
struct PipeIn {
    pipe: Registered<mio::unix::pipe::Receiver>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

impl super::io::Host for PipeIn {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self.pipe.attempt(Direction::Read, |pipe| (&*pipe).read(buffer)) {
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                Err(super::net::would_block(&mut self.waiting, self.gave_up))
            }
            read => read,
        }
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.waiting).then(|| Ready::Socket(self.pipe.ready(Direction::Read)))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

/// The end of a child's input pipe that the program writes. What the pipe
/// does not take at once waits here, as a socket's bytes do, and goes when
/// the pipe takes more: at the next write or flush, or at the poller's
/// event, also once the stream is gone, after which the pipe closes. A
/// write that finds bytes still waiting waits for the pipe itself.
struct PipeOut {
    outgoing: Arc<Outgoing>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

struct Outgoing {
    pipe: Registered<mio::unix::pipe::Sender>,
    unsent: Mutex<Unsent>,
}

#[derive(Default)]
struct Unsent {
    bytes: Vec<u8>,
    /// Sending failed, as to a child that is gone: every write after says
    /// so.
    failed: Option<ErrorKind>,
    /// The stream let go of the pipe with bytes to send: the pipe closes
    /// after them, and is held until then.
    keep: Option<Arc<Outgoing>>,
}

impl Outgoing {
    /// How much of `data` the pipe takes now.
    fn send(&self, data: &[u8]) -> std::io::Result<usize> {
        let mut sent = 0;
        while sent < data.len() {
            match self.pipe.attempt(Direction::Write, |pipe| quietly(pipe, &data[sent..])) {
                Ok(count) => sent += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(sent)
    }

    /// Sends what waits while the pipe takes it.
    fn push(&self, unsent: &mut Unsent) -> std::io::Result<()> {
        if let Some(kind) = unsent.failed {
            return Err(kind.into());
        }
        match self.send(&unsent.bytes) {
            Ok(sent) => {
                unsent.bytes.drain(..sent);
                Ok(())
            }
            Err(error) => {
                unsent.bytes.clear();
                unsent.failed = Some(error.kind());
                Err(error)
            }
        }
    }

    /// When the pipe can take more: sends what waits, and closes a pipe
    /// whose stream went once nothing does.
    fn writable(&self) {
        let mut unsent = self.unsent.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.push(&mut unsent);
        if unsent.keep.is_some() && (unsent.bytes.is_empty() || unsent.failed.is_some()) {
            let keep = unsent.keep.take();
            drop(unsent);
            drop(keep);
        }
    }
}

impl super::io::Host for PipeOut {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut unsent = self.outgoing.unsent.lock().unwrap_or_else(|e| e.into_inner());
        self.outgoing.push(&mut unsent)?;
        if !unsent.bytes.is_empty() {
            return Err(super::net::would_block(&mut self.waiting, self.gave_up));
        }
        match self.outgoing.send(data) {
            Ok(sent) => {
                unsent.bytes.extend_from_slice(&data[sent..]);
                Ok(())
            }
            Err(error) => {
                unsent.failed = Some(error.kind());
                Err(error)
            }
        }
    }

    /// Waits until the pipe took everything.
    fn flush(&mut self) -> std::io::Result<()> {
        let mut unsent = self.outgoing.unsent.lock().unwrap_or_else(|e| e.into_inner());
        self.outgoing.push(&mut unsent)?;
        if unsent.bytes.is_empty() {
            return Ok(());
        }
        Err(super::net::would_block(&mut self.waiting, self.gave_up))
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.waiting).then(|| Ready::Socket(self.outgoing.pipe.ready(Direction::Write)))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl Drop for PipeOut {
    fn drop(&mut self) {
        let mut unsent = self.outgoing.unsent.lock().unwrap_or_else(|e| e.into_inner());
        if !unsent.bytes.is_empty() && unsent.failed.is_none() {
            unsent.keep = Some(self.outgoing.clone());
        }
    }
}

/// A write to a pipe whose child may be gone fails with `BrokenPipe` and
/// raises no `SIGPIPE`, which would end the program, as Go's writes to any
/// file but the standard output and error do. macOS lets a pipe say so
/// once; Linux has no such flag for a pipe, so the thread blocks the signal
/// for the write and takes back the one the write raised.
#[cfg(target_os = "macos")]
fn quiet(pipe: &mio::unix::pipe::Sender) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    match unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) } {
        -1 => Err(std::io::Error::last_os_error()),
        _ => Ok(()),
    }
}

#[cfg(not(target_os = "macos"))]
fn quiet(_pipe: &mio::unix::pipe::Sender) -> std::io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn quietly(pipe: &mio::unix::pipe::Sender, data: &[u8]) -> std::io::Result<usize> {
    (&*pipe).write(data)
}

#[cfg(not(target_os = "macos"))]
fn quietly(pipe: &mio::unix::pipe::Sender, data: &[u8]) -> std::io::Result<usize> {
    unsafe {
        let mut only: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut only);
        libc::sigaddset(&mut only, libc::SIGPIPE);
        // One already pending is the program's, and stays.
        let mut pending: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut pending);
        let was_pending = libc::sigismember(&pending, libc::SIGPIPE) == 1;
        let mut before: libc::sigset_t = std::mem::zeroed();
        libc::pthread_sigmask(libc::SIG_BLOCK, &only, &mut before);
        let written = (&*pipe).write(data);
        if !was_pending && written.as_ref().is_err_and(|error| error.kind() == ErrorKind::BrokenPipe) {
            let now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            libc::sigtimedwait(&only, std::ptr::null_mut(), &now);
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &before, std::ptr::null_mut());
        written
    }
}
