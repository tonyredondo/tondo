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
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

/// The table the compiler writes: one entry per function of the program,
/// whose `meta` offset in `blob` holds its name's length, its number of
/// calls, the name and, 4-aligned, the calls as `[return offset, line,
/// column, file, type]`. At `files` in `blob`: the count of the names the
/// calls refer to, their files and the types they assign, and each one's
/// length and text, 4-aligned. No type is `u32::MAX`.
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
/// The tables of a live build's patches, in the order they came.
const PATCHES: usize = 4096;
static PATCHED: [AtomicPtr<Table>; PATCHES] = [const { AtomicPtr::new(std::ptr::null_mut()) }; PATCHES];
static PATCHED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Registers the program's table; the first thing `main` does.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_trace(table: *const Table) {
    TABLE.store(table as *mut Table, Ordering::Release);
}

/// Registers a patch's table; false when there is no room for another.
pub(crate) fn add(table: *const Table) -> bool {
    let count = PATCHED_COUNT.load(Ordering::Acquire);
    if count == PATCHES {
        return false;
    }
    PATCHED[count].store(table as *mut Table, Ordering::Release);
    PATCHED_COUNT.store(count + 1, Ordering::Release);
    true
}

/// The program's table, then its patches'.
fn tables() -> impl Iterator<Item = (usize, &'static Table)> {
    let patched = PATCHED_COUNT.load(Ordering::Acquire);
    std::iter::once(&TABLE)
        .chain(&PATCHED[..patched])
        .enumerate()
        .filter_map(|(index, table)| unsafe { table.load(Ordering::Acquire).as_ref() }.map(|table| (index, table)))
}

/// A frame of compiled code: the function and the line it is at, and the
/// type the statement there assigns.
#[derive(PartialEq)]
struct Frame {
    function: &'static str,
    file: &'static str,
    line: u32,
    column: u32,
    ty: Option<&'static str>,
}

fn entries(table: &Table) -> &'static [Function] {
    unsafe { std::slice::from_raw_parts(table.functions, table.count as usize) }
}

/// Whether a table's functions lie in its order, as the compiler writes
/// them into one section, which linkers keep.
fn ordered(slot: usize, table: &Table) -> bool {
    static ORDERED: [AtomicU8; PATCHES + 1] = [const { AtomicU8::new(0) }; PATCHES + 1];
    match ORDERED[slot].load(Ordering::Relaxed) {
        0 => {
            let ordered = entries(table).windows(2).all(|pair| pair[0].code < pair[1].code);
            ORDERED[slot].store(if ordered { 1 } else { 2 }, Ordering::Relaxed);
            ordered
        }
        state => state == 1,
    }
}

/// The function a return address is in: its table, its start and its
/// entry.
fn find(address: usize) -> Option<(&'static Table, usize, usize)> {
    let inside = |f: &Function| f.code < address && address <= f.code + f.size as usize;
    tables().find_map(|(slot, table)| {
        let entries = entries(table);
        let index = if ordered(slot, table) {
            let at = entries.partition_point(|f| f.code < address).checked_sub(1)?;
            Some(at).filter(|&at| inside(&entries[at]))?
        } else {
            entries.iter().position(inside)?
        };
        Some((table, entries[index].code, index))
    })
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
    let (table, start, index) = find(address)?;
    unsafe {
        let blob = table.blob;
        let meta = (*table.functions.add(index)).meta as usize;
        let (name_len, calls) = (u32_at(blob, meta) as usize, u32_at(blob, meta + 4) as usize);
        let function = text_at(blob, meta + 8, name_len);
        let sites = aligned(meta + 8, name_len);
        let offset = (address - start) as u32;
        let site = (0..calls).map(|k| sites + k * 20).find(|&site| u32_at(blob, site) == offset)?;
        let name = |index: u32| {
            let mut cursor = table.files as usize + 4;
            for _ in 0..index {
                cursor = aligned(cursor + 4, u32_at(blob, cursor) as usize);
            }
            text_at(blob, cursor + 4, u32_at(blob, cursor) as usize)
        };
        let ty = u32_at(blob, site + 16);
        Some(Frame {
            function,
            file: name(u32_at(blob, site + 12)),
            line: u32_at(blob, site + 4),
            column: u32_at(blob, site + 8),
            ty: (ty != u32::MAX).then(|| name(ty)),
        })
    }
}

/// Where a walk of the stack from `fp` should start to reach compiled code:
/// at `fp`, unless no frame of compiled code follows it, because code
/// without frame pointers in between, as Rust's standard library, used the
/// register for its own values. Then the stack above is searched for the
/// first return to a call of compiled code, whose frame record lies below
/// it: a few kilobytes above, where the runtime's own calls end.
pub(crate) fn through(fp: usize, bounds: (usize, usize)) -> usize {
    if walk(fp, bounds).any(|address| resolve(address).is_some()) {
        return fp;
    }
    let start = fp.max(bounds.0).next_multiple_of(8);
    for slot in (start..bounds.1.saturating_sub(8)).step_by(8).take(1024) {
        let word = unsafe { *(slot as *const usize) };
        if resolve(word).is_some() {
            return slot - 8;
        }
    }
    fp
}

/// Whether the instruction at `pc` is compiled code's.
pub(crate) fn compiled(pc: usize) -> bool {
    find(pc + 1).is_some()
}

/// What a trace shows for an address of compiled code: the function, and
/// where the function calls another, the file and line and the type the
/// statement there assigns.
pub(crate) struct Described {
    pub function: &'static str,
    pub at: Option<(&'static str, u32)>,
    pub ty: Option<&'static str>,
}

pub(crate) fn describe(address: usize) -> Option<Described> {
    if let Some(frame) = resolve(address) {
        return Some(Described { function: frame.function, at: Some((frame.file, frame.line)), ty: frame.ty });
    }
    let (table, _, index) = find(address).or_else(|| find(address + 1))?;
    unsafe {
        let meta = (*table.functions.add(index)).meta as usize;
        let function = text_at(table.blob, meta + 8, u32_at(table.blob, meta) as usize);
        Some(Described { function, at: None, ty: None })
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

/// The return addresses of the running task's stack from here.
pub(crate) fn here_frames() -> Vec<usize> {
    here().collect()
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

impl Origin {
    /// The return addresses of the spawn, innermost first.
    pub(crate) fn frames(&self) -> &[usize] {
        match self {
            Origin::Spawned { addresses, count, .. } => &addresses[..*count],
            Origin::Foreign => &[],
        }
    }
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
    // The body of `spawn call()` only makes the call, and a trace shows the
    // spawn as the task's origin instead.
    let mut frames = addresses.filter_map(resolve).filter(|f| !spawn_body(f.function)).peekable();
    std::iter::from_fn(move || {
        let frame = frames.next()?;
        let mut times = 1;
        while frames.next_if(|next| *next == frame).is_some() {
            times += 1;
        }
        Some((frame, times))
    })
}

/// Whether a function is the body of a `spawn`, which the compiler names
/// `spawn` and the function it calls.
fn spawn_body(function: &str) -> bool {
    function == "spawn" || function.starts_with("spawn ")
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

/// Where the program's own code is in the running task's stack: the first
/// frame outside the standard library, which a panic inside it points at.
pub(crate) fn caller() -> Option<String> {
    let frame = here().filter_map(resolve).find(|frame| !frame.file.starts_with("<std>/"))?;
    Some(format!("{}:{}:{}", frame.file, frame.line, frame.column))
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

const FAULTS: [i32; 5] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGABRT, libc::SIGILL, libc::SIGFPE];

/// The handlers there were before, which in a library get the faults that
/// are not its own.
static PREVIOUS: std::sync::OnceLock<[libc::sigaction; 5]> = std::sync::OnceLock::new();

/// Prepares the running thread, which runs tasks, to report a fault of the
/// program: a task's stack that overflows, memory C touched that it should
/// not, an abort, an illegal instruction or an arithmetic fault, as C may
/// make. The first call installs the handler. A thread of C that calls a
/// library keeps the signal stack it has.
pub(crate) fn guard_thread(runtime: bool) {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = fault as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut action.sa_mask);
        let mut previous: [libc::sigaction; 5] = std::mem::zeroed();
        for (signal, previous) in FAULTS.into_iter().zip(&mut previous) {
            libc::sigaction(signal, &action, previous);
        }
        let _ = PREVIOUS.set(previous);
    });
    SIGNAL.with_borrow_mut(|signal| {
        if signal.is_some() {
            return;
        }
        unsafe {
            if !runtime {
                let mut current: libc::stack_t = std::mem::zeroed();
                libc::sigaltstack(std::ptr::null(), &mut current);
                if current.ss_flags & libc::SS_DISABLE == 0 {
                    return;
                }
            }
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

/// Hands a fault to the handler there was before, as if it had come to it.
unsafe fn forward(signal: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    let Some(index) = FAULTS.iter().position(|&s| s == signal) else { return };
    let Some(previous) = PREVIOUS.get().map(|p| p[index]) else { return };
    unsafe {
        match previous.sa_sigaction {
            libc::SIG_IGN => {}
            // The instruction faults again, or the abort comes again, and
            // ends the process as it would have.
            libc::SIG_DFL => {
                libc::sigaction(signal, &previous, std::ptr::null_mut());
                if signal == libc::SIGABRT {
                    libc::raise(signal);
                }
            }
            handler if previous.sa_flags & libc::SA_SIGINFO != 0 => {
                let handler: extern "C" fn(i32, *mut libc::siginfo_t, *mut libc::c_void) = std::mem::transmute(handler);
                handler(signal, info, context);
            }
            handler => {
                let handler: extern "C" fn(i32) = std::mem::transmute(handler);
                handler(signal);
            }
        }
    }
}

/// The fault's address, and the instruction, frame pointer and return
/// address of the code that faulted: the link register, or the top of the
/// stack, which holds one when a call was being made.
pub(crate) unsafe fn faulted(
    info: *const libc::siginfo_t,
    context: *const libc::c_void,
) -> (usize, usize, usize, usize) {
    unsafe {
        let context = context as *const libc::ucontext_t;
        #[cfg(target_os = "linux")]
        let address = (*info).si_addr() as usize;
        #[cfg(not(target_os = "linux"))]
        let address = (*info).si_addr as usize;
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let (pc, fp, link) = {
            let registers = &(*context).uc_mcontext.gregs;
            let link = top_word(registers[libc::REG_RSP as usize] as usize, address);
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
            (state.__rip as usize, state.__rbp as usize, top_word(state.__rsp as usize, address))
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let (pc, fp, link) = {
            let state = &(*(*context).uc_mcontext).__ss;
            (state.__pc as usize, state.__fp as usize, state.__lr as usize)
        };
        (address, pc, fp, link)
    }
}

/// The word at the top of the stack, or 0 when the stack pointer may be
/// past the stack's end, in its guard: a function's frame can take it
/// there before anything is written, and reading there would fault again
/// inside the handler.
#[cfg(target_arch = "x86_64")]
unsafe fn top_word(sp: usize, address: usize) -> usize {
    let readable = match crate::tasks::stack_bounds() {
        Some((bottom, top)) => (bottom..top.saturating_sub(8)).contains(&sp),
        None => sp != 0 && sp.abs_diff(address) >= crate::tasks::GUARD,
    };
    if readable { unsafe { *(sp as *const usize) } } else { 0 }
}

/// Reports a fault with the trace of the code that made it, and ends the
/// process as the signal does: the handler goes back to the default one,
/// and the instruction faults again, or the signal comes again.
extern "C" fn fault(signal: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    unsafe {
        let (address, pc, fp, link) = faulted(info, context);
        // In a library, a fault outside its tasks and code is its host's.
        if crate::library() && crate::tasks::current().is_null() && find(pc).is_none() {
            forward(signal, info, context);
            return;
        }
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
            let _ = match signal {
                libc::SIGABRT => write!(out, "fatal: SIGABRT: the program aborted"),
                libc::SIGILL => write!(out, "fatal: SIGILL: illegal instruction at {pc:#x}"),
                libc::SIGFPE => write!(out, "fatal: SIGFPE: arithmetic fault at {pc:#x}"),
                libc::SIGBUS => write!(out, "fatal: SIGBUS: invalid memory access at {address:#x}"),
                _ => write!(out, "fatal: SIGSEGV: invalid memory access at {address:#x}"),
            };
            if in_c {
                let _ = write!(out, ", inside a function of C");
            }
        }
        // A fault inside C starts the trace where the program called it;
        // one in compiled code, at the call it may have been making.
        let (fp, link) = match crate::tasks::c_frame() {
            Some(called) if in_c => called,
            _ => (fp, link),
        };
        let walk = walk(fp, bounds);
        let first = (resolve(link).is_some() && walk.clone().next() != Some(link)).then_some(link);
        let origin = if task.is_null() { None } else { crate::tasks::origin_of(task) };
        write_stack(&mut out, "stack", || first.into_iter().chain(walk.clone()), origin.as_deref());
        let _ = writeln!(out);
        out.flush();
        // A dump at a fault is as good as the memory it reads.
        let message = match signal {
            libc::SIGABRT => "SIGABRT: the program aborted".to_string(),
            libc::SIGILL => format!("SIGILL: illegal instruction at {pc:#x}"),
            libc::SIGFPE => format!("SIGFPE: arithmetic fault at {pc:#x}"),
            libc::SIGBUS => format!("SIGBUS: invalid memory access at {address:#x}"),
            _ => format!("SIGSEGV: invalid memory access at {address:#x}"),
        };
        let frames = std::iter::once(pc).chain(first).chain(walk).collect();
        crate::diagnostics::crash_dump("fault", &message, frames, false);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(signal, &action, std::ptr::null_mut());
        // An abort is not an instruction that faults again when the handler
        // returns: it is raised once more, and ends the process then.
        if signal == libc::SIGABRT {
            libc::raise(signal);
        }
    }
}
