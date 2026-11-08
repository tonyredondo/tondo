//! The recording on disk (docs/refactor.md, phase 6): its directory, where
//! the recordings of children go, its segments and how much of a run they
//! keep, and the copies of the program a debugger goes back to.

use super::*;

/// The directory of the recording a run makes or replays.
pub(super) static RECORDING: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The recording's directory, whole, as children that run elsewhere and
/// the debugger find it.
pub(super) fn directory_of(calls: &Path) {
    let calls = std::path::absolute(calls).unwrap_or_else(|_| calls.to_path_buf());
    let _ = RECORDING.set(calls.parent().unwrap_or(Path::new("/")).to_path_buf());
}

/// Where a child of the program records its run, when it is a debug build
/// of Tondo: a directory of its own there, named by its process.
pub(crate) fn children() -> Option<PathBuf> {
    RECORDING.get().map(|dir| dir.join("children"))
}

/// In a replay, the recording of the child `pid` started, if it made one.
pub(crate) fn recorded_child(pid: u32) -> Option<PathBuf> {
    let _outside = Outside::enter();
    let dir = children()?.join(pid.to_string());
    (replaying() && dir.join("run.json").is_file()).then_some(dir)
}

/// The recording of this run, a child's in `children`: the program, and
/// what it ran with, as `tondo run --record` writes them; the file its
/// calls go to.
pub(super) fn recording_of_child(children: &Path) -> Option<PathBuf> {
    let dir = children.join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::copy(std::env::current_exe().ok()?, dir.join("program")).ok()?;
    let quoted = |text: &std::ffi::OsStr| crate::diagnostics::quoted(&text.to_string_lossy());
    let (invoked, arguments) = crate::env::arguments().split_first()?;
    let arguments: Vec<String> = arguments.iter().map(|a| quoted(a)).collect();
    let environment: Vec<String> =
        std::env::vars_os().map(|(name, value)| format!("{}:{}", quoted(&name), quoted(&value))).collect();
    let seed = crate::config::config().schedule.clone().unwrap_or_else(|| "0".into());
    let run = format!(
        "{{\"source\":{},\"arguments\":[{}],\"environment\":{{{}}},\"directory\":{},\"seed\":{},\"tondo\":{}}}",
        quoted(invoked),
        arguments.join(","),
        environment.join(","),
        quoted(std::env::current_dir().unwrap_or_default().as_os_str()),
        crate::diagnostics::quoted(&seed),
        crate::diagnostics::quoted(env!("CARGO_PKG_VERSION")),
    );
    std::fs::write(dir.join("run.json"), run).ok()?;
    Some(dir.join("calls"))
}

/// The debugger that started this replay is gone.
pub(crate) fn orphaned() -> bool {
    let debugger = DEBUGGER.load(Ordering::Relaxed);
    if debugger <= 1 {
        return false;
    }
    let _outside = Outside::enter();
    // Gone, or ended and not yet waited for.
    #[cfg(target_os = "linux")]
    return match std::fs::read_to_string(format!("/proc/{debugger}/stat")) {
        Ok(stat) => stat.rsplit_once(") ").is_some_and(|(_, state)| state.starts_with(['Z', 'X'])),
        Err(_) => true,
    };
    #[cfg(not(target_os = "linux"))]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let found = unsafe { libc::proc_pidinfo(debugger, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        found != size || info.pbi_status == libc::SZOMB
    }
}

/// How much of its end a recording keeps, in milliseconds, or all of it.
pub(super) static LAST: AtomicU64 = AtomicU64::new(0);

/// How long a copy waits unused, in milliseconds, as a checkpoint does.
pub(super) const HOUR: u64 = 3_600_000;
pub(super) static HOLD_FOR: AtomicU64 = AtomicU64::new(HOUR);

pub(crate) fn hold_for() -> std::time::Duration {
    std::time::Duration::from_millis(HOLD_FOR.load(Ordering::Relaxed))
}

/// A copy of the program held at a probe, for `--last`: where the recording
/// goes on from, in the segment it started; the start of the run is one, in
/// no process.
pub(super) struct Copied {
    pub(super) tick: u64,
    pub(super) pid: i32,
    pub(super) segment: u32,
    pub(super) at: std::time::Instant,
}

pub(super) static COPIES: Mutex<Vec<Copied>> = Mutex::new(Vec::new());
/// The last time a copy was taken, which the next is due after.
pub(super) static TAKEN: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The file of a segment of the recording: the first is `calls`.
pub(super) fn segment_path(calls: &Path, segment: u32) -> PathBuf {
    match segment {
        0 => calls.to_path_buf(),
        _ => calls.with_extension(segment.to_string()),
    }
}

/// The calls of a recording from a segment on, to its last.
pub(super) fn calls_from(calls: &Path, first: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for segment in first.. {
        let path = segment_path(calls, segment);
        let file = match std::fs::read(&path) {
            Ok(file) => file,
            Err(_) if segment > first => break,
            Err(e) => return Err(format!("cannot read the recording {}: {e}", path.display())),
        };
        let blocks = file.strip_prefix(MAGIC).and_then(unpacked);
        out.extend(blocks.ok_or("the file is not a recording of this tondo")?);
    }
    Ok(out)
}

/// Whether a copy of the program is due, as the end of the recording moves
/// on, looked at every so many probes. A copy would lose threads of C, so a
/// run that has them keeps it all.
#[inline]
pub(crate) fn copy_due() -> bool {
    static LOOK: AtomicU64 = AtomicU64::new(0);
    let last = LAST.load(Ordering::Relaxed);
    if last == 0 || !look_at(&LOOK, 1 << 12) || !PROGRAM.get() || threaded() {
        return false;
    }
    let _outside = Outside::enter();
    let now = std::time::Instant::now();
    let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    let at = *taken.get_or_insert(now);
    at != now && now.duration_since(at).as_millis() as u64 >= last / 2
}

/// A copy of the program here, and a new segment after it: `copied` runs in
/// the copy, which only comes back to replay what follows, and takes the
/// segment it starts at. Copies and segments the end of the run no longer
/// needs go. The log is held across the fork, so that the copy has it free.
pub(crate) fn copy(copied: impl FnOnce(u32)) {
    let Some(calls) = RECORDING.get().map(|dir| dir.join("calls")) else { return };
    store(true);
    let mut held = COPIES.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    *TAKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
    if held.is_empty() {
        held.push(Copied { tick: 0, pid: 0, segment: 0, at: now });
    }
    let segment = held.last().map_or(1, |last| last.segment + 1);
    let Ok(mut file) = std::fs::File::create(segment_path(&calls, segment)) else { return };
    if std::io::Write::write_all(&mut file, MAGIC).is_err() {
        return;
    }
    let mut log = log();
    let baton = baton();
    let pid = crate::tasks::fork_copy();
    if pid == 0 {
        drop(baton);
        drop(file);
        log.file = None;
        drop(log);
        drop(held);
        unsafe { libc::setsid() };
        copied(segment);
        return;
    }
    drop(baton);
    if pid < 0 {
        return;
    }
    log.file = Some(file);
    drop(log);
    held.push(Copied { tick: TICKS.load(Ordering::Relaxed), pid, segment, at: now });
    // The oldest goes once the next is old enough to cover the window.
    let window = std::time::Duration::from_millis(LAST.load(Ordering::Relaxed));
    while held.len() > 1 && now.duration_since(held[1].at) >= window {
        let gone = held.remove(0);
        if gone.pid != 0 {
            unsafe {
                libc::kill(gone.pid, libc::SIGKILL);
                libc::waitpid(gone.pid, std::ptr::null_mut(), 0);
            }
            let _ = std::fs::remove_file(crate::diagnostics::sockets().join(format!("{}.checkpoint", gone.pid)));
        }
        for segment in gone.segment..held[0].segment {
            let _ = std::fs::remove_file(segment_path(&calls, segment));
        }
    }
    prune_children(&calls, window);
    let text: String = held.iter().map(|h| format!("{} {}\n", h.tick, h.pid)).collect();
    let (list, written) = (calls.with_file_name("held"), calls.with_file_name("held.new"));
    if std::fs::write(&written, text).is_ok() {
        let _ = std::fs::rename(&written, list);
    }
}

/// The recordings of children that wrote nothing for as long as the end of
/// the run is kept go: they started before it, and no replay reaches them.
pub(super) fn prune_children(calls: &Path, window: std::time::Duration) {
    let children = calls.with_file_name("children");
    let now = std::time::SystemTime::now();
    let old = |dir: &Path| {
        let written = std::fs::metadata(dir.join("calls")).and_then(|calls| calls.modified());
        written.is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age >= window))
    };
    for child in std::fs::read_dir(children).into_iter().flatten().flatten() {
        if old(&child.path()) {
            let _ = std::fs::remove_dir_all(child.path());
        }
    }
}

/// A copy goes on as a replay, of what was recorded since it was taken.
pub(crate) fn replay_from(segment: u32, debugger: i32) {
    let Some(calls) = RECORDING.get().map(|dir| dir.join("calls")) else { return };
    let recorded = calls_from(&calls, segment).unwrap_or_else(|message| crate::runtime_fault(&message));
    let mut log = log();
    log.recorded = recorded;
    log.at = 0;
    LAST.store(0, Ordering::Relaxed);
    DEBUGGER.store(debugger, Ordering::Relaxed);
    MODE.store(REPLAY, Ordering::Relaxed);
}

/// Writes what the recording holds so far, where a crash may have left
/// anything held: as it is.
pub(crate) fn flush() {
    store(false);
}

/// Writes what the recording holds so far as a block, compressed if asked.
pub(super) fn store(compressed: bool) {
    use std::io::Write;
    let mut log = log();
    let Log { file, buffer, .. } = &mut *log;
    if let Some(file) = file.as_mut().filter(|_| !buffer.is_empty()) {
        let _outside = Outside::enter();
        let packed = compressed.then(|| packed(buffer)).flatten().filter(|packed| packed.len() < buffer.len());
        let stored = packed.as_ref().unwrap_or(buffer);
        let _ = file.write_all(&(buffer.len() as u32).to_le_bytes());
        let _ = file.write_all(&(stored.len() as u32).to_le_bytes());
        let _ = file.write_all(stored);
        buffer.clear();
    }
}

pub(super) fn packed(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).ok()?;
    encoder.finish().ok()
}

/// The calls of the blocks of a recording; none if it is not one, but for a
/// last block a crash cut short, which is lost as what it held would be.
pub(super) fn unpacked(mut blocks: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    while let Some((head, rest)) = blocks.split_first_chunk::<8>() {
        let length = u32::from_le_bytes(head[..4].try_into().ok()?) as usize;
        let stored = u32::from_le_bytes(head[4..].try_into().ok()?) as usize;
        let Some((block, rest)) = rest.split_at_checked(stored) else { break };
        match stored == length {
            true => out.extend_from_slice(block),
            false => {
                flate2::read::DeflateDecoder::new(block).read_to_end(&mut out).ok()?;
            }
        }
        blocks = rest;
    }
    Some(out)
}
