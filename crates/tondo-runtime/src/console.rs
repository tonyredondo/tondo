//! The console (docs/refactor.md, phase 5): standard output, an ordered buffer
//! delivered when it fills, at a line when it is a terminal, and as the
//! program ends.

use super::*;

/// Standard output. `print` and `println` append to this ordered buffer
/// (std.console); it is delivered when it fills, at every line when stdout
/// is a terminal or the program a debug build, by `flush`, and when the
/// program ends, so programs never need to flush.
pub(crate) struct Output {
    pub(crate) buffer: Vec<u8>,
    /// Whether stdout is a terminal, decided on first use.
    terminal: Option<bool>,
    /// The first delivery failure nobody has observed yet. Output written
    /// after a failure is dropped.
    failed: Option<std::io::ErrorKind>,
}

pub(crate) static STDOUT: Mutex<Output> = Mutex::new(Output { buffer: Vec::new(), terminal: None, failed: None });
pub(crate) const STDOUT_FLUSH_THRESHOLD: usize = 64 * 1024;

impl Output {
    fn deliver(&mut self) {
        if self.failed.is_none() && !self.buffer.is_empty() {
            let mut stdout = std::io::stdout().lock();
            if let Err(error) = stdout.write_all(&self.buffer).and_then(|()| stdout.flush()) {
                self.failed = Some(error.kind());
            }
        }
        self.buffer.clear();
    }
}

pub(crate) fn stdout() -> std::sync::MutexGuard<'static, Output> {
    STDOUT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The standard output, unless another holds it.
pub(crate) fn try_stdout() -> Option<std::sync::MutexGuard<'static, Output>> {
    match STDOUT.try_lock() {
        Ok(out) => Some(out),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

pub(crate) fn console_append(bytes: &[u8], newline: bool) {
    let mut out = stdout();
    out.buffer.extend_from_slice(bytes);
    if newline {
        out.buffer.push(b'\n');
    }
    let terminal = *out.terminal.get_or_insert_with(|| std::io::IsTerminal::is_terminal(&std::io::stdout()));
    let line = newline || bytes.contains(&b'\n');
    // A test's output must survive a crash, so a test build delivers it at
    // once; a debug or live build's comes at every line, as to a terminal, for
    // the tool that reads it.
    let lines = terminal || debugger::built() || live::on();
    if out.buffer.len() >= STDOUT_FLUSH_THRESHOLD || (lines && line) || testing::ACTIVE.load(Ordering::Relaxed) {
        out.deliver();
    } else {
        STDOUT_WAITS.store(true, Ordering::Relaxed);
    }
}

/// Output is waiting in the buffer: a program with nothing left to do lets it go.
pub(crate) static STDOUT_WAITS: AtomicBool = AtomicBool::new(false);

/// Whether there is output to deliver, which says so once.
pub(crate) fn stdout_waits() -> bool {
    STDOUT_WAITS.swap(false, Ordering::Relaxed)
}

/// Delivers pending output before the process reports a failure itself.
pub fn flush_stdout() {
    stdout().deliver();
}

/// The first failure to deliver standard output since the last call.
pub(crate) fn stdout_failure() -> std::io::Result<()> {
    stdout().failed.take().map_or(Ok(()), |kind| Err(kind.into()))
}

/// Delivers pending output when the program ends. A failure nobody observed
/// is reported on stderr and makes the process exit with status 1.
pub fn finish_stdout() -> Option<i32> {
    let failed = io::finish();
    let mut out = stdout();
    out.deliver();
    let kind = failed.or(out.failed.take())?;
    let variant = match kind {
        std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe => "Closed",
        std::io::ErrorKind::InvalidData => "InvalidData",
        _ => "Host",
    };
    let _ = writeln!(std::io::stderr(), "error: standard output: io.IoError.{variant}");
    Some(1)
}
