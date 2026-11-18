//! The console (docs/refactor.md, phase 5): standard output, an ordered buffer
//! delivered when it fills, at a line when it is a terminal, and as the
//! program ends.

use super::*;
use std::sync::atomic::AtomicU64;

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
    /// The live region at the bottom of a terminal (std.cli): the text of
    /// each of its parts, in the order they came, by number.
    region: Vec<(u64, String)>,
    /// The lines of it on the screen now: what a write clears first.
    drawn: usize,
    /// Whether the cursor is in the middle of a line the program wrote, as
    /// a question's prompt, below which no region is drawn until it ends.
    open: bool,
}

pub(crate) static STDOUT: Mutex<Output> =
    Mutex::new(Output { buffer: Vec::new(), terminal: None, failed: None, region: Vec::new(), drawn: 0, open: false });
pub(crate) const STDOUT_FLUSH_THRESHOLD: usize = 64 * 1024;

impl Output {
    fn deliver(&mut self) {
        if self.drawn > 0 || (!self.region.is_empty() && self.terminal == Some(true)) {
            return self.redraw();
        }
        let bytes = std::mem::take(&mut self.buffer);
        self.write(bytes);
    }

    fn write(&mut self, bytes: Vec<u8>) {
        if self.failed.is_none() && !bytes.is_empty() {
            let mut stdout = std::io::stdout().lock();
            if let Err(error) = stdout.write_all(&bytes).and_then(|()| stdout.flush()) {
                self.failed = Some(error.kind());
            }
        }
    }

    /// Delivers what waits and draws the region below it, in one write so
    /// that nothing flickers: the region drawn before goes first, and comes
    /// back only below a whole line, not after a prompt.
    fn redraw(&mut self) {
        let mut bytes = self.cleared();
        if !self.buffer.is_empty() {
            self.open = self.buffer.last() != Some(&b'\n');
            bytes.append(&mut self.buffer);
        }
        if !self.open && !self.region.is_empty() {
            let columns = super::terminal::size().map_or(80, |(columns, _)| columns as usize);
            let lines: Vec<String> = self
                .region
                .iter()
                .flat_map(|(_, text)| text.lines())
                .map(|line| clipped(line, columns.saturating_sub(1).max(1)))
                .collect();
            self.drawn = lines.len();
            bytes.extend_from_slice(lines.join("\n").as_bytes());
        }
        self.write(bytes);
    }

    /// What takes the region off the screen, leaving the cursor where it
    /// started.
    fn cleared(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        if self.drawn > 1 {
            bytes.extend_from_slice(format!("\r\x1b[{}A", self.drawn - 1).as_bytes());
        }
        if self.drawn > 0 {
            bytes.extend_from_slice(b"\r\x1b[J");
        }
        self.drawn = 0;
        bytes
    }
}

/// A line of the region, cut to `room` columns of the terminal, its styles'
/// codes kept and closed.
fn clipped(line: &str, room: usize) -> String {
    let (mut out, mut used, mut escaping, mut styled) = (String::new(), 0, false, false);
    for c in line.chars() {
        if escaping {
            escaping = c != 'm';
        } else if c == '\x1b' {
            escaping = true;
            styled = true;
        } else {
            let wide = super::text::columns(c);
            if used + wide > room {
                break;
            }
            used += wide;
        }
        out.push(c);
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// The parts of the live region, which ends a part when its value goes
/// (std.cli's `Live` and progress).
pub(crate) struct RegionPart(u64);

static PARTS: AtomicU64 = AtomicU64::new(1);

/// A new part of the live region, empty until shown.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_region_new() -> *mut super::native::NativeObject {
    super::native::new(RegionPart(PARTS.fetch_add(1, Ordering::Relaxed)))
}

/// Shows `text` as the part: on a terminal at once, in place, and
/// elsewhere only when it ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_region_show(part: *mut super::native::NativeObject, text: *const Str) {
    let Some(&RegionPart(id)) = super::native::get::<RegionPart>(part) else { return };
    let text = String::from_utf8_lossy(unsafe { str_bytes(&text) }).into_owned();
    let mut out = stdout();
    out.terminal.get_or_insert_with(|| std::io::IsTerminal::is_terminal(&std::io::stdout()));
    match out.region.iter_mut().find(|(known, _)| *known == id) {
        Some((_, shown)) => *shown = text,
        None => out.region.push((id, text)),
    }
    if out.terminal == Some(true) {
        out.redraw();
    }
}

/// Ends the part, its last text written above the region, and to the
/// output when it is no terminal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_region_end(part: *mut super::native::NativeObject) {
    if let Some(&RegionPart(id)) = super::native::get::<RegionPart>(part) {
        end_part(&mut stdout(), id);
    }
}

fn end_part(out: &mut Output, id: u64) {
    let Some(at) = out.region.iter().position(|(known, _)| *known == id) else { return };
    let (_, text) = out.region.remove(at);
    if !text.is_empty() {
        out.buffer.extend_from_slice(text.as_bytes());
        out.buffer.push(b'\n');
    }
    out.deliver();
}

impl Drop for RegionPart {
    fn drop(&mut self) {
        // As the program ends the region may be another's to finish.
        if let Some(mut out) = try_stdout() {
            end_part(&mut out, self.0);
        }
    }
}

/// Draws the region again, as at a new size of the terminal.
pub(crate) fn region_redraw() {
    let mut out = stdout();
    if !out.region.is_empty() && out.terminal == Some(true) {
        out.redraw();
    }
}

/// Before a prompt reads a line: the cursor is at its end, and the region
/// comes back below the line typed once it is read.
pub(crate) fn region_read() {
    let mut out = stdout();
    if out.open && !out.region.is_empty() {
        out.open = false;
        out.redraw();
    }
}

/// Writes what a part of the region last showed as the program ends, a
/// panic too, with the cursor below it: no screen is left half drawn.
pub(crate) fn region_finish() {
    let Some(mut out) = try_stdout() else { return };
    let parts: Vec<u64> = out.region.iter().map(|(id, _)| *id).collect();
    for id in parts {
        end_part(&mut out, id);
    }
}

/// Before standard error writes on a terminal: the region leaves the
/// screen, and comes back below what it wrote.
pub(crate) fn region_around(write: impl FnOnce()) {
    let Some(mut out) = try_stdout() else { return write() };
    if out.drawn == 0 {
        drop(out);
        return write();
    }
    let cleared = out.cleared();
    out.write(cleared);
    write();
    out.redraw();
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
