//! `std.process` (standard library 19.2): plans of commands and pipelines,
//! the processes they start, and how their streams connect. A plan is an
//! immutable shared object, so `Command` and `Pipeline` copy by reference.

use super::io::{Mode, StreamObject, byte_array, host_cancellable, replaced_console, retain, run, stream};
use super::{ArrayHeader, LIVE_OBJECTS, Str, element, str_bytes, tondo_rt_array_new};
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

const SIGKILL: i32 = 9;
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
    OsStr::from_bytes(unsafe { str_bytes(text) }).to_owned()
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
    }
}

/// `process.command(program, arguments...)`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_command(program: *const Str, arguments: *mut ArrayHeader) -> *mut PlanObject {
    let arguments =
        (0..unsafe { (*arguments).len }).map(|i| os(unsafe { *(element(arguments, 8, i) as *const *const Str) }));
    plan(vec![stage(os(program), arguments.collect())])
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
/// 6 `stderr` (with the tag of the `Input` or `Output` and its payload), and
/// 7 `mergeStderr()`. `stdin` is the first stage's, `stdout` the last's.
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
        _ => stages.iter_mut().for_each(|s| s.merge = true),
    }
    plan(stages)
}

/// An `Input` from its tag and payload.
unsafe fn input(tag: i64, payload: *const u8) -> Input {
    match tag {
        0 => Input::Inherit,
        1 => Input::Null,
        2 => Input::Bytes(Arc::new(unsafe { str_bytes(payload as *const Str) }.to_vec())),
        3 => Input::Bytes(Arc::new(unsafe { super::io::array_bytes(payload as *mut ArrayHeader) })),
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
    streams: [Option<Held>; 3],
    workers: Vec<JoinHandle<()>>,
    captured: Arc<Mutex<Captured>>,
    statuses: Option<Vec<(i64, bool)>>,
    detached: bool,
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

/// Copies what a child writes into the captured output and `combined`.
fn collect(mut pipe: std::io::PipeReader, captured: Arc<Mutex<Captured>>, stage: Option<usize>) -> JoinHandle<()> {
    super::tasks::spawn_thread(None, move || {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let mut captured = captured.lock().unwrap_or_else(|e| e.into_inner());
            let chunk = &buffer[..count];
            match stage {
                None => captured.stdout.extend_from_slice(chunk),
                Some(stage) => captured.stderr[stage].extend_from_slice(chunk),
            }
            captured.combined.extend_from_slice(chunk);
        }
    })
}

/// Copies what a child writes into a stream of `std.io`.
fn forward(mut pipe: std::io::PipeReader, target: Held) -> JoinHandle<()> {
    super::tasks::spawn_thread(None, move || {
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
    })
}

/// Feeds a child's input from bytes or a stream of `std.io`.
fn feed(mut pipe: std::io::PipeWriter, input: Input) -> JoinHandle<()> {
    super::tasks::spawn_thread(None, move || match input {
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
    })
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
            running.streams[standard] = Some(Held(stream(Box::new(reader), Mode::Long, false)));
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
        streams: [None, None, None],
        workers: Vec::new(),
        captured: Arc::new(Mutex::new(Captured { stderr: vec![Vec::new(); stages.len()], ..Captured::default() })),
        statuses: None,
        detached: false,
    };
    // Output written so far reaches the terminal before the children's.
    super::flush_stdout();
    let environment = super::env::environment();
    let mut previous: Option<std::io::PipeReader> = None;
    let started = (|| {
        for (index, stage) in stages.iter().enumerate() {
            let last = index + 1 == stages.len();
            let mut command = std::process::Command::new(&stage.program);
            command.args(&stage.arguments).env_clear();
            if !stage.clear_env {
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
                            running.streams[0] = Some(Held(stream(Box::new(writer), Mode::Long, false)));
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
            let child = command.spawn()?;
            super::diagnostics::PROCESSES_STARTED.add(1);
            running.pids.push(child.id());
            running.children.push(child);
            // `command` drops here, closing this side's copies of the pipes.
        }
        Ok(())
    })();
    if let Err(error) = started {
        kill_all(&running.pids);
        let _ = finish(&mut running);
        return Err(error);
    }
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).extend(running.pids.iter().copied());
    Ok(running)
}

/// The children of processes that started and nobody waited for, which a
/// panic cancels.
static LIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// How many processes the program started are running.
pub(crate) fn running() -> usize {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn kill_all(pids: &[u32]) {
    for &pid in pids {
        unsafe { kill(pid as i32, SIGKILL) };
    }
}

/// Cancels the processes still running when a panic ends the program.
pub(crate) fn cancel_live() {
    let pids = std::mem::take(&mut *LIVE.lock().unwrap_or_else(|e| e.into_inner()));
    kill_all(&pids);
}

/// Waits for every stage and the work around them; the statuses as
/// `(code, success)`.
fn finish(running: &mut Running) -> Vec<(i64, bool)> {
    if let Some(statuses) = &running.statuses {
        return statuses.clone();
    }
    // The program's side of a piped input closes, so the child sees its end.
    running.streams[0] = None;
    let statuses: Vec<(i64, bool)> = running
        .children
        .iter_mut()
        .map(|child| match child.wait() {
            Ok(status) => match (status.code(), status.signal()) {
                (Some(code), _) => (code as i64, code == 0),
                (None, Some(signal)) => (128 + signal as i64, false),
                _ => (1, false),
            },
            Err(_) => (1, false),
        })
        .collect();
    for worker in running.workers.drain(..) {
        let _ = worker.join();
    }
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|pid| !running.pids.contains(pid));
    running.statuses = Some(statuses.clone());
    statuses
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

/// Writes a `ProcessOutput` as four words, then the overall status as two:
/// stdout, stderr, combined, statuses, code and success.
unsafe fn write_output(running: &mut Running, layout: StatusLayout, out: *mut i64) -> (i64, bool) {
    let statuses = finish(running);
    let (code, success) = overall(&statuses);
    let captured = running.captured.lock().unwrap_or_else(|e| e.into_inner());
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
    (code, success)
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
    let pids = running.pids.clone();
    let statuses = host_cancellable(move || finish(&mut running), || kill_all(&pids));
    let (code, success) = overall(&statuses);
    unsafe { std::ptr::copy_nonoverlapping([code, success as i64].as_ptr(), out, 2) };
    0
}

/// `output()` and `check()`: a `ProcessOutput` into `out` (see
/// `write_output`); `check` fails with `Failed` when a stage did.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_output(
    plan: *mut PlanObject,
    check: u8,
    size: i64,
    success_offset: i64,
    out: *mut i64,
) -> i64 {
    let layout = StatusLayout { size: size as usize, success: success_offset as usize };
    let mut running = match spawn(plan_ref(plan), Defaults::Capture) {
        Ok(running) => running,
        Err(error) => return error_status(&error),
    };
    let pids = running.pids.clone();
    let out = out as usize;
    let (_, success) =
        host_cancellable(move || unsafe { write_output(&mut running, layout, out as *mut i64) }, || kill_all(&pids));
    if check != 0 && !success { FAILED } else { 0 }
}

/// A started process.
#[repr(C)]
pub struct ProcessObject {
    rc: AtomicUsize,
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
    match spawn(plan_ref(plan), Defaults::Inherit) {
        Ok(running) => {
            LIVE_OBJECTS.made();
            let process = ProcessObject { rc: AtomicUsize::new(1), running: Mutex::new(running) };
            unsafe { *out = Box::into_raw(Box::new(process)) };
            0
        }
        Err(error) => error_status(&error),
    }
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
            let closed = super::io::tondo_rt_io_discard();
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
    Process(process).running().pids.last().map_or(0, |&pid| pid as i64)
}

/// Waits for the process, with `cancel` killing it first; a cancelled task
/// kills it too.
fn settle<R: Send + 'static>(
    process: *mut ProcessObject,
    cancel: bool,
    then: impl FnOnce(&mut Running) -> R + Send + 'static,
) -> R {
    let pids = Process(process).running().pids.clone();
    if cancel {
        kill_all(&pids);
    }
    let shared = Process(process);
    host_cancellable(move || then(&mut shared.running()), || kill_all(&pids))
}

/// `wait()` and `cancel()` (`cancel` 1): the overall status into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_wait(process: *mut ProcessObject, cancel: u8, out: *mut i64) -> i64 {
    let statuses = settle(process, cancel != 0, finish);
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
    let out = out as usize;
    let (_, success) = settle(process, false, move |running| unsafe { write_output(running, layout, out as *mut i64) });
    if check != 0 && !success { FAILED } else { 0 }
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
        streams: [running.streams[0].take(), running.streams[1].take(), running.streams[2].take()],
        workers: std::mem::take(&mut running.workers),
        captured: running.captured.clone(),
        statuses: None,
        detached: true,
    };
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|pid| !gone.pids.contains(pid));
    super::tasks::spawn_thread(None, move || {
        // The program's side of a piped input closes first.
        drop(std::mem::take(&mut gone.streams));
        for child in &mut gone.children {
            let _ = child.wait();
        }
        for worker in gone.workers.drain(..) {
            let _ = worker.join();
        }
    });
}

/// Releases a `Process`: the last reference closes its input and waits for
/// it, unless it was detached.
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
        if !Process(process).running().detached {
            settle(process, false, |running| {
                finish(running);
            });
        }
        drop(Box::from_raw(process));
        LIVE_OBJECTS.gone();
    }
}

/// `ProcessOutput.text()`: the stdout as UTF-8 into `out`, or `InvalidData`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_process_text(bytes: *mut ArrayHeader, out: *mut *mut Str) -> i64 {
    match String::from_utf8(unsafe { super::io::array_bytes(bytes) }) {
        Ok(text) => {
            unsafe { *out = super::str_new(text.as_bytes()) };
            0
        }
        Err(_) => INVALID_DATA,
    }
}

/// `process.exit(code)`: delivers the console's output and ends the program.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_process_exit(code: i64) -> ! {
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
        _ => "host".to_string(),
    };
    unsafe { (*b).0.extend_from_slice(text.as_bytes()) }
}
