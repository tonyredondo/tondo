//! Stack traces (language 15.7): compiled code and the runtime keep the
//! chain of frame pointers, and the program's table maps the return address
//! of each call to its function and source line. A task keeps the trace of
//! where it was spawned, and its spawner's, so a report shows how the
//! program got there across tasks and threads.
//!
//! Reading a trace allocates nothing, so a fault can report one from a
//! signal handler.

use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

/// The table the compiler writes: one entry per function of the program,
/// whose `meta` offset in `blob` holds its name's length, its number of
/// calls, the name and, 4-aligned, the calls as `[return offset, line,
/// column, file]`. At `files` in `blob`: the count of files and each one's
/// length and name, 4-aligned.
#[repr(C)]
pub struct Table {
    functions: *const Function,
    count: u64,
    blob: *const u8,
    files: u64,
}

#[repr(C)]
struct Function {
    code: usize,
    size: u32,
    meta: u32,
}

static TABLE: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());

/// Registers the program's table; the first thing `main` does.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_trace(table: *const Table) {
    TABLE.store(table as *mut Table, Ordering::Release);
}

/// A frame of compiled code: the function and the line it is at.
#[derive(PartialEq)]
struct Frame {
    function: &'static str,
    file: &'static str,
    line: u32,
    column: u32,
}

fn entries() -> &'static [Function] {
    let table = TABLE.load(Ordering::Acquire);
    if table.is_null() {
        return &[];
    }
    unsafe { std::slice::from_raw_parts((*table).functions, (*table).count as usize) }
}

/// Whether the functions lie in the order of the table, as the compiler
/// writes them into one section, which linkers keep.
fn ordered() -> bool {
    static ORDERED: AtomicU8 = AtomicU8::new(0);
    match ORDERED.load(Ordering::Relaxed) {
        0 => {
            let ordered = entries().windows(2).all(|pair| pair[0].code < pair[1].code);
            ORDERED.store(if ordered { 1 } else { 2 }, Ordering::Relaxed);
            ordered
        }
        state => state == 1,
    }
}

/// The function a return address is in: its start and entry.
fn find(address: usize) -> Option<(usize, usize)> {
    let entries = entries();
    let inside = |f: &Function| f.code < address && address <= f.code + f.size as usize;
    let index = if ordered() {
        let at = entries.partition_point(|f| f.code < address).checked_sub(1)?;
        Some(at).filter(|&at| inside(&entries[at]))?
    } else {
        entries.iter().position(inside)?
    };
    Some((entries[index].code, index))
}

unsafe fn u32_at(blob: *const u8, offset: usize) -> u32 {
    unsafe { (blob.add(offset) as *const u32).read_unaligned() }
}

unsafe fn text_at(blob: *const u8, offset: usize, len: usize) -> &'static str {
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(blob.add(offset), len)) }
}

/// The 4-aligned end of `len` bytes from `offset`.
fn aligned(offset: usize, len: usize) -> usize {
    (offset + len + 3) & !3
}

/// The frame of compiled code a return address belongs to.
fn resolve(address: usize) -> Option<Frame> {
    let (start, index) = find(address)?;
    unsafe {
        let table = &*TABLE.load(Ordering::Acquire);
        let blob = table.blob;
        let meta = (*table.functions.add(index)).meta as usize;
        let (name_len, calls) = (u32_at(blob, meta) as usize, u32_at(blob, meta + 4) as usize);
        let function = text_at(blob, meta + 8, name_len);
        let sites = aligned(meta + 8, name_len);
        let offset = (address - start) as u32;
        let site = (0..calls).map(|k| sites + k * 16).find(|&site| u32_at(blob, site) == offset)?;
        let file = u32_at(blob, site + 12) as usize;
        let mut cursor = table.files as usize + 4;
        for _ in 0..file {
            cursor = aligned(cursor + 4, u32_at(blob, cursor) as usize);
        }
        Some(Frame {
            function,
            file: text_at(blob, cursor + 4, u32_at(blob, cursor) as usize),
            line: u32_at(blob, site + 4),
            column: u32_at(blob, site + 8),
        })
    }
}

/// The frame pointer of the function this inlines into.
#[inline(always)]
pub(crate) fn frame_pointer() -> usize {
    let fp: usize;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        std::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack, preserves_flags));
        #[cfg(target_arch = "aarch64")]
        std::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags));
    }
    fp
}

/// The return addresses on a stack from the frame `fp` on, innermost
/// first. Each frame record holds the caller's frame pointer and the return
/// address; frame pointers grow towards the top of the stack, and anything
/// else ends the walk.
#[derive(Clone)]
pub(crate) struct Walk {
    fp: usize,
    bottom: usize,
    top: usize,
}

pub(crate) fn walk(fp: usize, (bottom, top): (usize, usize)) -> Walk {
    Walk { fp, bottom, top }
}

impl Iterator for Walk {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let fp = self.fp;
        if fp < self.bottom || fp + 16 > self.top || !fp.is_multiple_of(8) {
            return None;
        }
        let (next, address) = unsafe { (*(fp as *const usize), *((fp + 8) as *const usize)) };
        self.fp = if next > fp { next } else { 0 };
        (address != 0).then_some(address)
    }
}

/// The running task's stack from here.
fn here() -> Walk {
    let fp = frame_pointer();
    walk(fp, crate::tasks::stack_bounds().unwrap_or((0, 0)))
}

/// Where a task was started: the trace of its spawn and its spawner's
/// origin, or a thread of C that called back. Always behind an `Arc`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Origin {
    Spawned { addresses: [usize; SPAWN_DEPTH], count: usize, parent: Option<Arc<Origin>>, depth: usize },
    Foreign,
}

/// The frames a spawn keeps, in the origin itself: one allocation a spawn.
const SPAWN_DEPTH: usize = 30;

/// The origins a chain keeps: a pipeline of tasks that each spawn the next
/// would otherwise hold all of them.
const ORIGINS: usize = 16;

/// The origin of a task the running one spawns from here.
#[inline(always)]
pub(crate) fn spawned(parent: Option<Arc<Origin>>) -> Arc<Origin> {
    let mut addresses = [0; SPAWN_DEPTH];
    let mut count = 0;
    for (slot, address) in addresses.iter_mut().zip(here()) {
        *slot = address;
        count += 1;
    }
    let depth = match parent.as_deref() {
        Some(Origin::Spawned { depth, .. }) => depth + 1,
        _ => 1,
    };
    let parent = parent.filter(|_| depth < ORIGINS);
    // A loop spawns from the same place many times: one origin serves all.
    thread_local! {
        static LAST: std::cell::RefCell<Option<Arc<Origin>>> = const { std::cell::RefCell::new(None) };
    }
    LAST.with_borrow_mut(|last| {
        if let Some(found) = last
            && let Origin::Spawned { addresses: a, count: c, parent: p, .. } = &**found
            && a[..*c] == addresses[..count]
            && p.as_ref().map(Arc::as_ptr) == parent.as_ref().map(Arc::as_ptr)
        {
            return found.clone();
        }
        let origin = Arc::new(Origin::Spawned { addresses, count, parent, depth });
        *last = Some(origin.clone());
        origin
    })
}

/// The runs of the same frame of compiled code in a stack, innermost
/// first, as in a recursion: each frame and how many times it repeats.
fn runs(addresses: impl Iterator<Item = usize>) -> impl Iterator<Item = (Frame, usize)> {
    let mut frames = addresses.filter_map(resolve).peekable();
    std::iter::from_fn(move || {
        let frame = frames.next()?;
        let mut times = 1;
        while frames.next_if(|next| *next == frame).is_some() {
            times += 1;
        }
        Some((frame, times))
    })
}

/// The lines a long trace shows from its start and from its end.
const HEAD: usize = 40;
const TAIL: usize = 20;

/// Writes the lines of a trace under `title`, each on a new line, if it
/// has frames of compiled code; a long one without its middle.
fn write_lines<I: Iterator<Item = usize>>(out: &mut dyn Write, title: &str, addresses: impl Fn() -> I) {
    let (lines, width) = runs(addresses()).fold((0, 0), |(n, w), (f, _)| (n + 1, w.max(f.function.len())));
    if lines == 0 {
        return;
    }
    let _ = write!(out, "\n{title}:");
    let mut hidden = 0;
    for (index, (frame, times)) in runs(addresses()).enumerate() {
        if lines > HEAD + TAIL && (HEAD..lines - TAIL).contains(&index) {
            hidden += times;
            if index == lines - TAIL - 1 {
                let _ = write!(out, "\n  ... {hidden} more frames");
            }
            continue;
        }
        let _ = write!(out, "\n  {:width$}  {}:{}:{}", frame.function, frame.file, frame.line, frame.column);
        if times > 1 {
            let _ = write!(out, "  ({times} times)");
        }
    }
}

/// Writes a stack under `title`, and where its task was started.
fn write_stack<I: Iterator<Item = usize>>(
    out: &mut dyn Write,
    title: &str,
    addresses: impl Fn() -> I,
    origin: Option<&Origin>,
) {
    write_lines(out, title, addresses);
    let mut origin = origin;
    while let Some(found) = origin {
        match found {
            Origin::Spawned { addresses, count, parent, depth } => {
                write_lines(out, "in a task spawned at", || addresses[..*count].iter().copied());
                if parent.is_none() && *depth >= ORIGINS {
                    let _ = write!(out, "\n(and {ORIGINS} or more tasks before it)");
                }
                origin = parent.as_deref();
            }
            Origin::Foreign => {
                let _ = write!(out, "\nin a callback that a thread of C made");
                origin = None;
            }
        }
    }
}

/// The trace of the running task from here, and where it was started, as
/// the lines that follow a report.
pub(crate) fn report() -> String {
    let walk = here();
    stack(walk, crate::tasks::origin().as_deref(), "stack")
}

/// A stack and the origins of its task, under `title`.
pub(crate) fn stack(walk: Walk, origin: Option<&Origin>, title: &str) -> String {
    let mut out = String::new();
    write_stack(&mut out, title, || walk.clone(), origin);
    out
}

// ---------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------

/// What reports a fault writes to: standard error, through a buffer on the
/// stack, since a signal handler cannot allocate.
struct Stderr {
    buffer: [u8; 512],
    len: usize,
}

impl Stderr {
    fn flush(&mut self) {
        write_all(2, &self.buffer[..self.len]);
        self.len = 0;
    }
}

/// Writes to a file descriptor, as a signal handler may.
fn write_all(fd: i32, bytes: &[u8]) {
    let mut written = 0;
    while written < bytes.len() {
        let n = unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
}

impl Write for Stderr {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        for &byte in text.as_bytes() {
            if self.len == self.buffer.len() {
                self.flush();
            }
            self.buffer[self.len] = byte;
            self.len += 1;
        }
        Ok(())
    }
}

/// The stack a thread reports a fault on: its own stack may be the one
/// that is full.
const SIGNAL_STACK: usize = 128 * 1024;

struct SignalStack(*mut libc::c_void);

impl Drop for SignalStack {
    fn drop(&mut self) {
        unsafe {
            let disabled = libc::stack_t { ss_sp: std::ptr::null_mut(), ss_flags: libc::SS_DISABLE, ss_size: 0 };
            libc::sigaltstack(&disabled, std::ptr::null_mut());
            libc::munmap(self.0, SIGNAL_STACK);
        }
    }
}

thread_local! {
    static SIGNAL: std::cell::RefCell<Option<SignalStack>> = const { std::cell::RefCell::new(None) };
}

/// Prepares the running thread, which runs tasks, to report a fault of the
/// program: a task's stack that overflows, or memory C touched that it
/// should not. The first call installs the handler.
pub(crate) fn guard_thread() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = fault as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGSEGV, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGBUS, &action, std::ptr::null_mut());
    });
    SIGNAL.with_borrow_mut(|signal| {
        if signal.is_some() {
            return;
        }
        unsafe {
            let flags = libc::MAP_PRIVATE | libc::MAP_ANON;
            let stack =
                libc::mmap(std::ptr::null_mut(), SIGNAL_STACK, libc::PROT_READ | libc::PROT_WRITE, flags, -1, 0);
            if stack == libc::MAP_FAILED {
                return;
            }
            let alternate = libc::stack_t { ss_sp: stack, ss_flags: 0, ss_size: SIGNAL_STACK };
            libc::sigaltstack(&alternate, std::ptr::null_mut());
            *signal = Some(SignalStack(stack));
        }
    });
}

/// The fault's address, and the instruction, frame pointer and return
/// address of the code that faulted: the link register, or the top of the
/// stack, which holds one when a call was being made.
unsafe fn faulted(info: *const libc::siginfo_t, context: *const libc::c_void) -> (usize, usize, usize, usize) {
    unsafe {
        let context = context as *const libc::ucontext_t;
        #[cfg(target_os = "linux")]
        let address = (*info).si_addr() as usize;
        #[cfg(not(target_os = "linux"))]
        let address = (*info).si_addr as usize;
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let (pc, fp, link) = {
            let registers = &(*context).uc_mcontext.gregs;
            let sp = registers[libc::REG_RSP as usize] as usize;
            let link = if sp == 0 { 0 } else { *(sp as *const usize) };
            (registers[libc::REG_RIP as usize] as usize, registers[libc::REG_RBP as usize] as usize, link)
        };
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        let (pc, fp, link) = {
            let state = &(*context).uc_mcontext;
            (state.pc as usize, state.regs[29] as usize, state.regs[30] as usize)
        };
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        let (pc, fp, link) = {
            let state = &(*(*context).uc_mcontext).__ss;
            let link = if state.__rsp == 0 { 0 } else { *(state.__rsp as *const usize) };
            (state.__rip as usize, state.__rbp as usize, link)
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let (pc, fp, link) = {
            let state = &(*(*context).uc_mcontext).__ss;
            (state.__pc as usize, state.__fp as usize, state.__lr as usize)
        };
        (address, pc, fp, link)
    }
}

/// Reports a fault with the trace of the code that made it, and ends the
/// process as the signal does: the handler goes back to the default one,
/// and the instruction faults again.
extern "C" fn fault(signal: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    unsafe {
        let (address, pc, fp, link) = faulted(info, context);
        // The output the program wrote so far comes first.
        if let Some(mut stdout) = crate::try_stdout() {
            write_all(1, &stdout.buffer);
            stdout.buffer.clear();
        }
        let mut out = Stderr { buffer: [0; 512], len: 0 };
        let task = crate::tasks::current();
        let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
        let in_c = find(pc).is_none() && crate::tasks::c_frame().is_some();
        if (bounds.0.saturating_sub(crate::tasks::GUARD)..bounds.0).contains(&address) {
            let size = (bounds.1 - bounds.0 + crate::tasks::GUARD) >> 20;
            let whose = if !task.is_null() && crate::tasks::is_main(task) { "the main task" } else { "a task" };
            let _ = write!(out, "fatal: stack overflow: {whose} used all of its {size} MiB stack");
        } else {
            let name = if signal == libc::SIGBUS { "SIGBUS" } else { "SIGSEGV" };
            let _ = write!(out, "fatal: {name}: invalid memory access at {address:#x}");
            if in_c {
                let _ = write!(out, ", inside a function of C");
            }
        }
        // A fault inside C starts the trace where the program called it;
        // one in compiled code, at the call it may have been making.
        let (fp, link) = match crate::tasks::c_frame() {
            Some(frame) if in_c => (frame, 0),
            _ => (fp, link),
        };
        let walk = walk(fp, bounds);
        let first = (resolve(link).is_some() && walk.clone().next() != Some(link)).then_some(link);
        let origin = if task.is_null() { None } else { crate::tasks::origin_of(task) };
        write_stack(&mut out, "stack", || first.into_iter().chain(walk.clone()), origin.as_deref());
        let _ = writeln!(out);
        out.flush();
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(signal, &action, std::ptr::null_mut());
    }
}
