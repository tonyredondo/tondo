//! Tracing (docs/refactor.md, phase 6): the session of `tondo trace`, which
//! gets the runtime's events and the frames they name.

use super::*;

/// The tool that traces the program, and the addresses it knows the frames
/// of.
pub(super) struct Session {
    pub(super) stream: UnixStream,
    pub(super) fd: i32,
    pub(super) described: std::collections::HashSet<usize>,
}

pub(super) static SESSION: Mutex<Option<Session>> = Mutex::new(None);
pub(super) static TRACED: std::sync::Condvar = std::sync::Condvar::new();

/// Starts tracing for a tool and tells it so, or says why not. The reply
/// goes while the session is held, so that the end of the trace, which the
/// program's exit writes from another thread, cannot reach the tool first.
pub(super) fn trace(client: &mut UnixStream) -> Result<(), String> {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session.is_some() {
        return Err("{\"error\":\"another tool traces this program\"}".into());
    }
    let Ok(stream) = client.try_clone() else { return Err("{\"error\":\"cannot trace\"}".into()) };
    *session = Some(Session { stream, fd: client.as_raw_fd(), described: Default::default() });
    if !crate::recorder::on() {
        crate::recorder::start();
    }
    let _ = send(client, &format!("{{\"tracing\":true,\"clock\":{}}}", crate::recorder::now()));
    TRACED.notify_all();
    Ok(())
}

/// Writes a line to a tool in one write, so that no other line can come
/// between it and its end.
pub(super) fn send(stream: &mut impl std::io::Write, line: &str) -> std::io::Result<()> {
    let mut whole = String::with_capacity(line.len() + 1);
    whole.push_str(line);
    whole.push('\n');
    stream.write_all(whole.as_bytes())
}

/// Sends the tool what was recorded since the last time; a tool that is
/// gone ends the trace.
pub(super) fn push(session: &mut Option<Session>) {
    let Some(tracer) = session.as_mut() else { return };
    let line = recorded(tracer);
    if send(&mut tracer.stream, &line).is_err() {
        crate::recorder::stop();
        *session = None;
    }
}

/// Ends the trace: what is left goes to the tool, and then why it ended;
/// as the program exits, with its memory, which is not there to ask for
/// afterwards.
pub(super) fn end_trace(session: &mut Option<Session>, why: &str) {
    if session.is_none() {
        return;
    }
    crate::recorder::stop();
    push(session);
    if let Some(mut tracer) = session.take() {
        let mut frames = Frames { described: std::mem::take(&mut tracer.described), json: String::new() };
        let memory = if why == "exit" { memory(&mut frames) } else { "null".into() };
        let ended = format!("{{\"end\":\"{why}\",\"memory\":{memory},\"frames\":{{{}}}}}", frames.json);
        let _ = send(&mut tracer.stream, &ended);
    }
}

/// The samples of the memory alive and what each stack allocated in all,
/// as JSON (toolchain 7.15).
pub(super) fn memory(frames: &mut Frames) -> String {
    let (samples, dropped) = crate::heap::alive();
    let (allocated, unstacked) = crate::heap::allocations();
    let mut out =
        format!("{{\"rate\":{},\"dropped\":{dropped},\"unstacked\":{unstacked},\"samples\":[", crate::heap::rate());
    for (index, (size, stack)) in samples.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ = write!(out, "{comma}{{\"size\":{size},\"stack\":{}}}", frames.list(stack));
    }
    out.push_str("],\"allocated\":[");
    for (index, (objects, bytes, stack)) in allocated.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ =
            write!(out, "{comma}{{\"objects\":{objects:.1},\"bytes\":{bytes:.0},\"stack\":{}}}", frames.list(stack));
    }
    out.push_str("]}");
    out
}

/// A line of JSON with the events and samples recorded since the last one,
/// and the frames of the addresses they hold that the tool does not know.
pub(super) fn recorded(session: &mut Session) -> String {
    use crate::recorder::{Event, Stop};
    let (events, dropped) = crate::recorder::take_events();
    let (samples, lost) = crate::recorder::take_samples();
    let mut frames = Frames { described: std::mem::take(&mut session.described), json: String::new() };
    let mut stack = |addresses: &[usize]| frames.list(addresses);
    let mut out = String::from("{\"events\":[");
    for (index, (at, thread, event)) in events.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ = match event {
            Event::Run { task } => write!(out, "{comma}[\"run\",{at},{thread},{task}]"),
            Event::Stop { task, why: Stop::Done } => write!(out, "{comma}[\"stop\",{at},{thread},{task},\"done\"]"),
            Event::Stop { task, why: Stop::Yields } => {
                write!(out, "{comma}[\"stop\",{at},{thread},{task},\"yields\"]")
            }
            Event::Stop { task, why: Stop::Waits(at_stack) } => {
                let frames = stack(at_stack);
                write!(out, "{comma}[\"stop\",{at},{thread},{task},\"waits\",{frames}]")
            }
            Event::Spawn { task, parent, code, at: from } => {
                let code = stack(&[*code + 1]);
                let frames = stack(from);
                write!(out, "{comma}[\"spawn\",{at},{thread},{task},{parent},{code},{frames}]")
            }
            Event::Pause { took } => write!(out, "{comma}[\"pause\",{at},{thread},{took}]"),
            Event::Call { task, took, at: from } => {
                let frames = stack(from);
                write!(out, "{comma}[\"call\",{at},{thread},{task},{took},{frames}]")
            }
            Event::Panic { task, message } => {
                write!(out, "{comma}[\"panic\",{at},{thread},{task},{}]", quoted(message))
            }
        };
    }
    out.push_str("],\"samples\":[");
    let mut labels = Labels::default();
    for (index, (at, thread, sampled, set)) in samples.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let frames = stack(sampled.frames());
        let _ = write!(out, "{comma}[{at},{thread},{frames},{}]", labels.number(*set));
    }
    session.described = std::mem::take(&mut frames.described);
    let _ = write!(
        out,
        "],\"labels\":{{{}}},\"frames\":{{{}}},\"dropped\":{dropped},\"lost\":{lost}}}",
        labels.json, frames.json
    );
    out
}

/// What the tool shows for an address: a function of the program, with its
/// file and line where it calls another; or else the file loaded there, the
/// program's own as `""`, and how far into it the address is, which the
/// tool finds in the file's symbols.
pub(super) fn frame(address: usize) -> String {
    if let Some(described) = crate::trace::describe(address) {
        let function = quoted(described.function);
        let ty = described.ty.map_or(String::new(), |ty| format!(",\"type\":{}", quoted(ty)));
        // The calls the code was inlined at, from the innermost out.
        let outer: Vec<String> = described
            .inlined
            .map(|(function, file, line, _)| {
                format!("{{\"function\":{},\"file\":{},\"line\":{line}}}", quoted(function), quoted(file))
            })
            .collect();
        let inlined = if outer.is_empty() { String::new() } else { format!(",\"inlined\":[{}]", outer.join(",")) };
        return match described.at {
            Some((file, line)) => {
                format!("{{\"function\":{function},\"file\":{},\"line\":{line}{ty}{inlined}}}", quoted(file))
            }
            None => format!("{{\"function\":{function}}}"),
        };
    }
    // A return address names the call before it.
    match loaded(address.saturating_sub(1)) {
        Some((image, offset)) => format!("{{\"image\":{},\"offset\":{offset}}}", quoted(&image)),
        None => "{}".into(),
    }
}

/// The file loaded where `address` is, and the address's offset from where
/// it was loaded. A static executable has no loader to ask: its code is all
/// the program's, loaded where its ELF header is.
pub(super) fn loaded(address: usize) -> Option<(String, usize)> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(address as *const libc::c_void, &mut info) } == 0 || info.dli_fname.is_null() {
        #[cfg(target_os = "linux")]
        {
            unsafe extern "C" {
                static __ehdr_start: u8;
            }
            return Some((String::new(), address.wrapping_sub(&raw const __ehdr_start as usize)));
        }
        #[cfg(not(target_os = "linux"))]
        return None;
    }
    let image = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }.to_string_lossy().into_owned();
    Some((image, address.wrapping_sub(info.dli_fbase as usize)))
}

pub(super) fn uptime_ms() -> u128 {
    STARTED.get().map_or(0, |started| started.elapsed().as_millis())
}

pub(super) fn info() -> String {
    let program = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let arguments: Vec<String> = crate::env::arguments().iter().skip(1).map(|a| quoted(&a.to_string_lossy())).collect();
    format!(
        "{{\"pid\":{},\"program\":{},\"arguments\":[{}],\"uptime_ms\":{},\"probes\":{}}}",
        std::process::id(),
        quoted(&program),
        arguments.join(","),
        uptime_ms(),
        crate::debugger::has_probes()
    )
}

pub(super) fn counters() -> String {
    format!("{{\"uptime_ms\":{},\"counters\":{}}}", uptime_ms(), counter_list())
}

/// The counters as a JSON array.
pub(super) fn counter_list() -> String {
    let mut out = String::from("[");
    for (index, sample) in sample().iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let kind = if sample.gauge { "gauge" } else { "counter" };
        let _ = write!(
            out,
            "{comma}{{\"name\":{},\"kind\":\"{kind}\",\"unit\":\"{}\",\"value\":{}}}",
            quoted(&sample.name),
            sample.unit,
            sample.value
        );
    }
    out.push(']');
    out
}
