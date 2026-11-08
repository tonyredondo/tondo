//! The server of the tools (docs/refactor.md, phase 6): the socket a program
//! listens on for them, started as it starts, and what each request asks.

use super::*;

pub(super) static STARTED: OnceLock<Instant> = OnceLock::new();
/// The program's socket, which a replay a checkpoint forked has its own of.
pub(super) static SOCKET: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(super) fn socket() -> Option<PathBuf> {
    SOCKET.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Where the processes of this user listen: `$TMPDIR/tondo-<uid>`.
pub(super) fn directory() -> PathBuf {
    std::env::temp_dir().join(format!("tondo-{}", unsafe { libc::geteuid() }))
}

/// Listens for the tools, unless `TONDO_DIAGNOSTICS=0`. A directory that
/// is not the user's alone, or a socket that cannot be made, leaves the
/// program without diagnostics and otherwise as it was.
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(crate) fn start() {
    STARTED.get_or_init(Instant::now);
    // Linked in, for gdb and lldb to call.
    std::hint::black_box(tondo_rt_dump_to as *const ());
    std::hint::black_box(crate::debugger::tondo_rt_debug_to as *const ());
    crate::recorder::thread();
    let config = config();
    if config.check_leaks.is_some() {
        hook(EXACT);
    }
    let on = crate::config::on;
    let debugged = on(&config.debug);
    crate::lockorder::start(debugged || asked_dump().is_some());
    if on(&config.race) {
        crate::race::start();
    }
    let live = crate::live::on() && config.diagnostics.as_ref().is_none_or(|value| value != "0");
    // `cpu`: a profile of processor time, which counting the memory would
    // change.
    let processor = config.trace.as_ref().is_some_and(|value| value == "cpu");
    let traced = on(&config.trace) || processor;
    if !on(&config.diagnostics) && !traced && !debugged && !live && asked_dump().is_none() {
        return;
    }
    if !processor || on(&config.diagnostics) {
        crate::heap::prepare();
        hook(WATCHED);
    }
    if !crate::replay::on() {
        schedule_dumps();
    }
    if asked_dump().is_some() {
        crate::events::start();
    }
    // `tondo debug -- program` debugs that program, not those it starts.
    if debugged {
        unsafe { std::env::remove_var("TONDO_DEBUG") };
        crate::debugger::WAIT.store(true, Ordering::Relaxed);
    }
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let directory = directory();
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&directory);
    let private = std::fs::symlink_metadata(&directory)
        .is_ok_and(|m| m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0);
    if !private {
        return;
    }
    let path = directory.join(std::process::id().to_string());
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    *SOCKET.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    // `atexit` is not a symbol of glibc's shared library.
    unsafe extern "C" {
        fn __cxa_atexit(function: extern "C" fn(*mut u8), argument: *mut u8, object: *mut u8) -> i32;
    }
    extern "C" fn remove(_: *mut u8) {
        // The tools' business, which a replay does again.
        let _outside = crate::replay::Outside::enter();
        end_trace(&mut SESSION.lock().unwrap_or_else(|e| e.into_inner()), "exit");
        crate::debugger::exited();
        if let Some(path) = socket() {
            let _ = std::fs::remove_file(path);
        }
    }
    unsafe { __cxa_atexit(remove, std::ptr::null_mut(), std::ptr::null_mut()) };
    // A runtime thread, which a deadlock does not take for one of C.
    if crate::tasks::try_spawn_thread(Some(128 * 1024), move || serve(listener)).is_err() {
        return;
    }
    // `tondo trace -- program` traces from the start: the program waits
    // for it, a while, before `main`. Its own programs are not traced.
    if traced {
        unsafe { std::env::remove_var("TONDO_TRACE") };
        crate::recorder::start();
        let session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        let waited = TRACED.wait_timeout_while(session, std::time::Duration::from_secs(5), |s| s.is_none());
        drop(waited);
    }
}

/// Answers every tool from this one thread: each line one sends, with a
/// line of JSON.
/// The directory of the programs' sockets, also a replay's checkpoints'.
pub(crate) fn sockets() -> PathBuf {
    directory()
}

/// A replay a checkpoint forked, whose parent's threads are gone: it
/// listens for the tools at a socket of its own.
pub(crate) fn listen_again() -> bool {
    let path = directory().join(std::process::id().to_string());
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return false };
    *SOCKET.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    // It lives for its debugger: nothing else would ever end it.
    let _ = crate::tasks::try_spawn_thread(Some(128 * 1024), move || {
        while !crate::replay::orphaned() {
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        let _ = std::fs::remove_file(&path);
        unsafe { libc::_exit(0) };
    });
    crate::tasks::try_spawn_thread(Some(128 * 1024), move || serve(listener)).is_ok()
}

pub(super) fn serve(listener: UnixListener) {
    retire_here();
    let _ = listener.set_nonblocking(true);
    let mut clients: Vec<(UnixStream, Vec<u8>)> = Vec::new();
    loop {
        let descriptors = std::iter::once(listener.as_raw_fd()).chain(clients.iter().map(|(c, _)| c.as_raw_fd()));
        let mut polled: Vec<libc::pollfd> =
            descriptors.map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
        // While a tool traces, what was recorded goes to it ten times a
        // second.
        let tracing = SESSION.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        let timeout = if tracing { 100 } else { -1 };
        if unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, timeout) } < 0 {
            continue;
        }
        let mut ready = polled[1..].iter().map(|p| p.revents != 0);
        clients.retain_mut(|(client, pending)| {
            let kept = !ready.next().unwrap_or(false) || answer(client, pending);
            if !kept {
                let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
                if session.as_ref().is_some_and(|s| s.fd == client.as_raw_fd()) {
                    crate::recorder::stop();
                    *session = None;
                }
                if crate::debugger::is_client(client.as_raw_fd()) {
                    crate::debugger::detach();
                }
                crate::logpoints::tool_gone(client.as_raw_fd());
            }
            kept
        });
        push(&mut SESSION.lock().unwrap_or_else(|e| e.into_inner()));
        if polled[0].revents != 0 {
            while let Ok((client, _)) = listener.accept() {
                let _ = client.set_nonblocking(false);
                let _ = client.set_write_timeout(Some(std::time::Duration::from_secs(1)));
                clients.push((client, Vec::new()));
            }
        }
    }
}

/// Reads what a tool sent and answers its whole lines; false once it is
/// gone, or sends a line longer than any request.
pub(super) fn answer(client: &mut UnixStream, pending: &mut Vec<u8>) -> bool {
    let mut buffer = [0; 1024];
    let count = match client.read(&mut buffer) {
        Ok(0) | Err(_) => return false,
        Ok(count) => count,
    };
    pending.extend_from_slice(&buffer[..count]);
    while let Some(end) = pending.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = pending.drain(..=end).collect();
        let line = String::from_utf8_lossy(&line).trim().to_string();
        let reply = match line.as_str() {
            _ if crate::debugger::is_client(client.as_raw_fd())
                && let Some(reply) = crate::debugger::command(&line) =>
            {
                reply
            }
            "debug" => crate::debugger::attach(client),
            "info" => info(),
            "sources" => format!("{{\"sources\":{}}}", quoted(&roots())),
            "requests" => {
                let mut frames = Frames::default();
                let flights = flights_json(&mut frames);
                format!("{{\"dump\":1{flights},\"frames\":{{{}}}}}", frames.json)
            }
            "live" => crate::live::describe(),
            "barrier" => crate::live::hold(),
            "stay" => crate::live::stay(),
            "held" => format!("{{\"held\":{}}}", crate::live::held_json()),
            "release" => crate::live::let_go(),
            _ if line.starts_with("again ") => crate::live::again(&line["again ".len()..]),
            _ if line.starts_with("patch ") => crate::live::patch(&line["patch ".len()..]),
            _ if line.starts_with("go ") => crate::logpoints::go(&line["go ".len()..]),
            _ if line.starts_with("values ") => crate::debugger::line_values_asked(&line["values ".len()..]),
            _ if line.starts_with("step ") => crate::logpoints::step(&line["step ".len()..], client),
            _ if line.starts_with("eval ") => crate::live::eval(&line["eval ".len()..], client),
            _ if line.starts_with("logpoint ") => crate::logpoints::set(&line["logpoint ".len()..], client),
            "counters" => counters(),
            "memory" => {
                let mut frames = Frames::default();
                let memory = memory(&mut frames);
                format!("{{\"memory\":{memory},\"frames\":{{{}}}}}", frames.json)
            }
            "trace" => match trace(client) {
                Ok(()) => continue,
                Err(reply) => reply,
            },
            "dump" => dump("request", "", None, Some(std::time::Duration::from_millis(250))),
            "stop" => {
                let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
                if session.as_ref().is_some_and(|s| s.fd == client.as_raw_fd()) {
                    end_trace(&mut session, "stop");
                    continue;
                }
                "{\"error\":\"this tool does not trace the program\"}".to_string()
            }
            other => format!("{{\"error\":{}}}", quoted(&format!("unknown request `{other}`"))),
        };
        // A logpoint answers by itself.
        if !reply.is_empty() && send(client, &reply).is_err() {
            return false;
        }
        crate::debugger::answered();
    }
    pending.len() < 4096
}
