//! Logpoints of a running build without probes (toolchain 7.5): the
//! compiler leaves at the start of each line, but in the loops that call
//! nothing, a call that is nothing. A logpoint writes that call, with every
//! thread of the program stopped, and takes it out again. The call saves
//! every register, so what the frame holds reads as at a fault, and a copy
//! of the process shows the values, as in a dump.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

/// A logpoint set: the sites of its line, what they held, how many times it
/// may yet be hit, and the tool that asked.
struct Logpoint {
    id: u64,
    sites: Vec<Site>,
    left: u64,
    client: UnixStream,
    /// What must hold for a time to count.
    condition: Option<String>,
    /// Where each time leaves a dump of the program, as it was there.
    dump: Option<String>,
    /// For how long, in seconds, each time holds its task, as a breakpoint
    /// of that task alone: the others go on.
    stop: Option<u64>,
    /// A text of the request a task must work for to count.
    request: Option<String>,
    /// A step of a task a logpoint holds: that logpoint, the task, and the
    /// frame it left: only that frame or those that called it count.
    step: Option<(u64, u64, usize)>,
}

/// Where a logpoint holds its task: its number, how long, the task, its frame,
/// its stack's bounds and the code of the calls it is in, innermost first.
struct Holding {
    id: u64,
    seconds: u64,
    task: u64,
    fp: usize,
    code: Vec<usize>,
}

static HOLDING: Mutex<Option<Holding>> = Mutex::new(None);

/// `step <id> next|out|in`: the task logpoint `id` holds goes on to the next
/// line of its function or of those that called it, of those alone, or of
/// any of the program's, which goes into its calls. What this writes it writes
/// when asked: the code as compiled stays as it is.
pub(crate) fn step(rest: &str, client: &UnixStream) -> String {
    use super::diagnostics::quoted;
    let error = |message: &str| format!("{{\"error\":{}}}", quoted(message));
    let mut words = rest.split_whitespace();
    let (id, how) = (words.next().and_then(|id| id.parse::<u64>().ok()), words.next());
    let held = HOLDING.lock().unwrap_or_else(|e| e.into_inner()).take_if(|held| Some(held.id) == id);
    let Some(held) = held else { return error("no task stands at this breakpoint") };
    let Ok(client) = client.try_clone() else { return error("the tool went away") };
    // The functions of the code it is in, by where each starts.
    let sites = super::debugger::latent_sites();
    let mut starts: Vec<usize> = sites.iter().map(|site| site.4).collect();
    starts.sort();
    starts.dedup();
    let function = |code: usize| starts.iter().rev().find(|&&start| start <= code).copied();
    let wanted: Vec<usize> = held.code.iter().filter_map(|&code| function(code)).collect();
    // Going out is to another function, which the code it was inlined in may
    // be: what tells them apart is where each place says it is.
    let named = |code: usize| {
        let scopes = super::trace::scopes_at(code);
        scopes.first().and_then(|(_, place)| place.rsplit(" in ").next().map(str::to_string))
    };
    let left = held.code.first().and_then(|&site| named(site + 1));
    let into = how == Some("in");
    let places: Vec<(usize, usize)> = sites
        .iter()
        .filter(|site| match into {
            true => !site.2.starts_with("<std>/"),
            false => wanted.contains(&site.4) && (how != Some("out") || named(site.0 + 1) != left),
        })
        .map(|site| (site.0, site.1))
        .collect();
    let writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    // The step before ended: what it wrote goes first, or taking it out later
    // would take this one's out too.
    let ended = lock().iter().filter(|logpoint| logpoint.step.is_some() && logpoint.left == 0).map(|l| l.id).collect();
    if !super::tasks::stop_world(std::time::Duration::from_secs(2)) {
        return error("the program did not stop for the step: try again");
    }
    take_out(ended);
    let written = write_calls(&places);
    super::tasks::resume_world();
    drop(writing);
    let sites = match written {
        Ok(sites) => sites,
        Err(message) => return error(&message),
    };
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Into a call is deeper than the frame it left.
    let step = Some((held.id, held.task, if into { 0 } else { held.fp }));
    let (dump, stop) = (Some("-".to_string()), Some(held.seconds));
    lock().push(Logpoint { id: next, sites, left: 1, client, condition: None, dump, stop, request: None, step });
    go(&held.id.to_string());
    String::new()
}

struct Site {
    address: usize,
    length: usize,
    original: Vec<u8>,
}

static SET: Mutex<Vec<Logpoint>> = Mutex::new(Vec::new());
/// The number of the next logpoint.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// One hit at a time: its registers go where a fault's do.
static HIT: Mutex<()> = Mutex::new(());
/// One change of the code at a time, from stopping the program to letting it
/// go: logpoints go from several threads as their tools leave, and one would
/// close a page to writing while another writes.
static WRITING: Mutex<()> = Mutex::new(());

/// The logpoints whose held task may go on, and what wakes it.
static GOING: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static GO: std::sync::Condvar = std::sync::Condvar::new();

/// `go <id>`: the task a logpoint holds goes on.
pub(crate) fn go(id: &str) -> String {
    if let Ok(id) = id.trim().parse() {
        GOING.lock().unwrap_or_else(|e| e.into_inner()).push(id);
        GO.notify_all();
    }
    String::new()
}

/// Holds this task at logpoint `id`, out of Tondo code so that collections
/// and dumps go on, until the tool says or the time ends.
fn held(id: u64, seconds: u64) {
    let paused = super::tasks::pause_here();
    let going = GOING.lock().unwrap_or_else(|e| e.into_inner());
    let time = std::time::Duration::from_secs(seconds);
    let (mut going, _) =
        GO.wait_timeout_while(going, time, |going| !going.contains(&id)).unwrap_or_else(|e| e.into_inner());
    going.retain(|&other| other != id);
    drop(going);
    if paused {
        super::tasks::enter_tondo();
    }
}

/// `logpoint <max> <file:line>[\t<condition>[\t<dump>[\t<seconds>[\t<request>]]]]`: tells
/// the tool its number and how many places the line has, or gives why not.
pub(crate) fn set(request: &str, client: &UnixStream) -> String {
    use super::diagnostics::quoted;
    let error = |message: &str| format!("{{\"error\":{}}}", quoted(message));
    let mut fields = request.split('\t');
    let request = fields.next().unwrap_or_default();
    let mut given = || fields.next().filter(|field| !field.is_empty()).map(str::to_string);
    let (condition, dump, stop) = (given(), given(), given().and_then(|seconds| seconds.parse().ok()));
    let serving = given();
    if let Some(Err(message)) = condition.as_deref().map(super::debugger::valid_expression) {
        return error(&format!("the condition: {message}"));
    }
    let Some((max, place)) = request.split_once(' ') else { return error("logpoint takes a limit and file:line") };
    let (Ok(max), Some((file, line))) = (max.parse::<u64>(), place.rsplit_once(':')) else {
        return error("logpoint takes a limit and file:line");
    };
    let Ok(line) = line.parse::<u32>() else { return error("logpoint takes a limit and file:line") };
    let sites = super::debugger::latent_sites();
    // A tool names a file from where it is, an editor by its whole path.
    let same_file = |site: &&(usize, usize, String, u32, usize)| {
        site.2 == file || site.2.ends_with(&format!("/{file}")) || file.ends_with(&format!("/{}", site.2))
    };
    let of_file: Vec<&(usize, usize, String, u32, usize)> = sites.iter().filter(same_file).collect();
    let places: Vec<(usize, usize)> =
        of_file.iter().filter(|site| site.3 == line).map(|site| (site.0, site.1)).collect();
    if places.is_empty() {
        let near = of_file.iter().map(|site| site.3).min_by_key(|known| known.abs_diff(line));
        return error(&match (of_file.is_empty(), near) {
            (true, _) => {
                format!("{file} has no places for a logpoint: this build has none, or the file is not its code")
            }
            (_, Some(near)) => format!(
                "line {line} has no place for a logpoint: it has no code, or is in a tight loop, which calls nothing; \
                 the nearest line that has one is {near}"
            ),
            _ => format!("line {line} has no place for a logpoint"),
        });
    }
    let Ok(mut client) = client.try_clone() else { return error("the tool went away") };
    let writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    if !super::tasks::stop_world(std::time::Duration::from_secs(2)) {
        return error("the program did not stop for the logpoint: try again");
    }
    let written = write_calls(&places);
    super::tasks::resume_world();
    drop(writing);
    let sites = match written {
        Ok(sites) => sites,
        Err(message) => return error(&message),
    };
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The answer goes first: a hit counts once it is in the list, and must
    // not reach the tool before it.
    let answered = format!("{{\"logpoint\":{id},\"places\":{}}}\n", sites.len());
    let _ = client.write_all(answered.as_bytes());
    lock().push(Logpoint { id, sites, left: max, client, condition, dump, stop, request: serving, step: None });
    String::new()
}

fn lock() -> std::sync::MutexGuard<'static, Vec<Logpoint>> {
    SET.lock().unwrap_or_else(|e| e.into_inner())
}

/// The calls of sites, which need the code to be writable for a moment.
fn write_calls(places: &[(usize, usize)]) -> Result<Vec<Site>, String> {
    let target = tondo_rt_site as *const () as usize;
    let mut sites = Vec::new();
    let mut edits = Vec::new();
    for &(address, length) in places {
        let original = unsafe { std::slice::from_raw_parts(address as *const u8, length) }.to_vec();
        edits.push((address, encoded_call(address, length, target)?));
        sites.push(Site { address, length, original });
    }
    patch(&edits)?;
    Ok(sites)
}

#[cfg(target_arch = "x86_64")]
fn encoded_call(address: usize, length: usize, target: usize) -> Result<Vec<u8>, String> {
    let distance = target as i64 - (address + 5) as i64;
    let distance = i32::try_from(distance).map_err(|_| "the runtime is too far from the program's code")?;
    if length != 5 {
        return Err("this place cannot take a call".into());
    }
    Ok([&[0xe8][..], &distance.to_le_bytes()].concat())
}

#[cfg(target_arch = "aarch64")]
fn encoded_call(address: usize, length: usize, target: usize) -> Result<Vec<u8>, String> {
    let distance = (target as i64 - address as i64) / 4;
    if length != 4 || !(-(1 << 25)..1 << 25).contains(&distance) {
        return Err("this place cannot take a call".into());
    }
    Ok((0x9400_0000u32 | (distance as u32 & 0x03ff_ffff)).to_le_bytes().to_vec())
}

/// Writes code, with every thread of the program stopped, on a system that
/// lets a process do so: the pages of all of it at once, as a step writes
/// many places.
fn patch(edits: &[(usize, Vec<u8>)]) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("a logpoint needs Linux: this system does not let a program change its code".into());
    }
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let Some(first) = edits.iter().map(|(address, _)| *address).min() else { return Ok(()) };
    let last = edits.iter().map(|(address, bytes)| address + bytes.len()).max().unwrap_or(first);
    let start = first & !(page - 1);
    let protect = |flags| unsafe { libc::mprotect(start as *mut libc::c_void, last - start, flags) };
    if protect(libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC) != 0 {
        return Err(format!(
            "this system does not let the program change its code: {}",
            std::io::Error::last_os_error()
        ));
    }
    for (address, bytes) in edits {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), *address as *mut u8, bytes.len()) };
    }
    protect(libc::PROT_READ | libc::PROT_EXEC);
    for (address, bytes) in edits {
        super::live::code_changed(*address, bytes.len());
    }
    Ok(())
}

/// Takes out the logpoint `id`.
fn remove(id: u64) {
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    if lock().iter().any(|logpoint| logpoint.id == id) {
        while !super::tasks::stop_world(std::time::Duration::from_secs(2)) {}
        take_out(vec![id]);
        super::tasks::resume_world();
    }
}

/// Puts back what logpoints wrote, with the program stopped.
fn take_out(ids: Vec<u64>) {
    let gone = {
        let mut set = lock();
        let (gone, kept) = std::mem::take(&mut *set).into_iter().partition(|logpoint| ids.contains(&logpoint.id));
        *set = kept;
        gone
    };
    let gone: Vec<Logpoint> = gone;
    let sites = gone.iter().flat_map(|logpoint| &logpoint.sites);
    let _ = patch(&sites.map(|site| (site.address, site.original.clone())).collect::<Vec<_>>());
}

/// Takes out the logpoints of a tool that went away.
pub(crate) fn tool_gone(client: i32) {
    use std::os::fd::AsRawFd;
    let same = |logpoint: &Logpoint| {
        let (mine, other) = (fd_identity(logpoint.client.as_raw_fd()), fd_identity(client));
        mine.is_some() && mine == other
    };
    let ids: Vec<u64> = lock().iter().filter(|logpoint| same(logpoint)).map(|logpoint| logpoint.id).collect();
    // A task it holds goes on with it.
    ids.iter().for_each(|id| drop(go(&id.to_string())));
    ids.into_iter().for_each(remove);
}

/// What a descriptor is open on, which its copies share.
fn fd_identity(fd: i32) -> Option<(u64, u64)> {
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(fd, &mut status) } == 0).then_some((status.st_dev as u64, status.st_ino as u64))
}

/// A site was reached: `saved` holds the registers as the trampoline left
/// them, and where the code goes on.
extern "C" fn hit(saved: *const usize) {
    let (registers, floats, address, fp) = unsafe { registers_of(saved) };
    // A site's call is its whole length before where it returns to.
    let found = {
        // The last set first: a step goes before what was on its line.
        let set = lock();
        set.iter().rev().find_map(|logpoint| {
            let site = logpoint.sites.iter().find(|site| site.address + site.length == address)?.address;
            let client = logpoint.client.try_clone().ok().filter(|_| logpoint.left > 0)?;
            // A step is of its task, out of what it left.
            if logpoint.step.is_some_and(|(_, task, frame)| task != super::tasks::current_id() || fp < frame) {
                return None;
            }
            let (condition, dump) = (logpoint.condition.clone(), logpoint.dump.clone());
            Some((logpoint.id, site, client, condition, dump, logpoint.stop, logpoint.request.clone(), logpoint.step))
        })
    };
    let Some((id, site, mut client, condition, dump, stop, request, step)) = found else { return };
    let serving = || super::tasks::request_of(super::tasks::current_id());
    if request.is_some_and(|text| !serving().is_some_and(|it| it.contains(&text))) {
        return;
    }
    // Those that come while one is shown go by.
    let Ok(_shown) = HIT.try_lock() else { return };
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    let values = super::debugger::frame_values(site, fp, bounds, &registers, &floats, condition);
    // A condition that does not hold leaves nothing, and takes no time of
    // the limit.
    if values.is_empty() {
        return;
    }
    let last = {
        let mut set = lock();
        let Some(logpoint) = set.iter_mut().find(|logpoint| logpoint.id == id && logpoint.left > 0) else { return };
        logpoint.left -= 1;
        logpoint.left == 0
    };
    let task = super::tasks::current_id();
    // In a file, or with `-` in the hit itself, for a tool that reads it
    // from another machine.
    let dumped = dump.map_or(String::new(), |path| {
        let here = std::iter::once(address).chain(super::trace::walk(fp, bounds)).collect();
        let text = super::diagnostics::dump_here(here, (site, fp, bounds), &registers, &floats);
        match path.as_str() {
            "-" => format!(",\"whole\":{text}"),
            _ if std::fs::write(&path, text).is_ok() => format!(",\"dump\":{}", super::diagnostics::quoted(&path)),
            _ => String::new(),
        }
    });
    // Where a task that stops is, which is what a debugger shows first.
    let stack = stop.map_or(String::new(), |seconds| {
        let here = std::iter::once(address).chain(super::trace::walk(fp, bounds));
        let places = here.flat_map(super::trace::scopes_at).map(|(_, place)| super::diagnostics::quoted(&place));
        format!(",\"stopped\":{seconds},\"stack\":[{}]", places.collect::<Vec<_>>().join(","))
    });
    // A step says the breakpoint it came from.
    let (shown, stepped) = step.map_or((id, ""), |(from, ..)| (from, ",\"step\":true"));
    let line =
        format!("{{\"hit\":{shown},\"task\":{task}{stepped}{dumped}{stack},\"values\":[{}]}}\n", values.join(","));
    if client.write_all(line.as_bytes()).is_err() {
        forget(id);
    } else if let Some(seconds) = stop {
        // It is not a step's to go on from: it is taken out next.
        if last {
            let _ = client.write_all(format!("{{\"done\":{id}}}\n").as_bytes());
            forget(id);
        }
        let code = std::iter::once(site).chain(super::trace::walk(fp, bounds)).collect();
        let holding = Holding { id: shown, seconds, task, fp, code };
        *HOLDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(holding);
        held(shown, seconds);
        HOLDING.lock().unwrap_or_else(|e| e.into_inner()).take_if(|held| held.id == shown && held.task == task);
        let _ = client.write_all(format!("{{\"went\":{shown}}}\n").as_bytes());
        return;
    }
    if last {
        let _ = client.write_all(format!("{{\"done\":{id}}}\n").as_bytes());
        forget(id);
    }
}

/// Takes a logpoint out from a thread of its own: this one is in the code
/// that stops, and would wait for itself.
fn forget(id: u64) {
    let _ = super::tasks::try_spawn_thread(None, move || remove(id));
}

/// The registers `tondo_rt_site` saved, by their DWARF numbers; those of
/// numbers, by theirs; where the code goes on; and the frame pointer.
#[cfg(target_arch = "x86_64")]
unsafe fn registers_of(saved: *const usize) -> ([usize; 32], [usize; 32], usize, usize) {
    // As it pushed them: the last first.
    const ORDER: [usize; 15] = [15, 14, 13, 12, 11, 10, 9, 8, 5, 4, 3, 2, 1, 0, 6];
    let mut registers = [0; 32];
    for (slot, &register) in ORDER.iter().enumerate() {
        registers[register] = unsafe { *saved.add(slot) };
    }
    registers[7] = saved as usize + 16 * 8;
    let mut floats = [0; 32];
    for (number, float) in floats.iter_mut().enumerate().take(16) {
        *float = unsafe { *saved.sub(32).add(number * 2) };
    }
    (registers, floats, unsafe { *saved.add(15) }, registers[6])
}

#[cfg(target_arch = "aarch64")]
unsafe fn registers_of(saved: *const usize) -> ([usize; 32], [usize; 32], usize, usize) {
    let mut registers = [0; 32];
    for (register, slot) in registers.iter_mut().enumerate().take(31) {
        *slot = unsafe { *saved.add(register) };
    }
    registers[31] = saved as usize + 768;
    let mut floats = [0; 32];
    for (number, float) in floats.iter_mut().enumerate() {
        *float = unsafe { *saved.add(32 + number * 2) };
    }
    (registers, floats, registers[30], registers[29])
}

/// What a site's call goes to. It keeps every register, those of numbers
/// too, as the call says it does.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn tondo_rt_site() {
    std::arch::naked_asm!(
        "push rbp",
        "push rax",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "sub rsp, 256",
        "movups [rsp], xmm0",
        "movups [rsp + 16], xmm1",
        "movups [rsp + 32], xmm2",
        "movups [rsp + 48], xmm3",
        "movups [rsp + 64], xmm4",
        "movups [rsp + 80], xmm5",
        "movups [rsp + 96], xmm6",
        "movups [rsp + 112], xmm7",
        "movups [rsp + 128], xmm8",
        "movups [rsp + 144], xmm9",
        "movups [rsp + 160], xmm10",
        "movups [rsp + 176], xmm11",
        "movups [rsp + 192], xmm12",
        "movups [rsp + 208], xmm13",
        "movups [rsp + 224], xmm14",
        "movups [rsp + 240], xmm15",
        "call {hit}",
        "movups xmm0, [rsp]",
        "movups xmm1, [rsp + 16]",
        "movups xmm2, [rsp + 32]",
        "movups xmm3, [rsp + 48]",
        "movups xmm4, [rsp + 64]",
        "movups xmm5, [rsp + 80]",
        "movups xmm6, [rsp + 96]",
        "movups xmm7, [rsp + 112]",
        "movups xmm8, [rsp + 128]",
        "movups xmm9, [rsp + 144]",
        "movups xmm10, [rsp + 160]",
        "movups xmm11, [rsp + 176]",
        "movups xmm12, [rsp + 192]",
        "movups xmm13, [rsp + 208]",
        "movups xmm14, [rsp + 224]",
        "movups xmm15, [rsp + 240]",
        "add rsp, 256",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rax",
        "pop rbp",
        "ret",
        hit = sym hit,
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn tondo_rt_site() {
    std::arch::naked_asm!(
        "sub sp, sp, #768",
        "stp x0, x1, [sp]",
        "stp x2, x3, [sp, #16]",
        "stp x4, x5, [sp, #32]",
        "stp x6, x7, [sp, #48]",
        "stp x8, x9, [sp, #64]",
        "stp x10, x11, [sp, #80]",
        "stp x12, x13, [sp, #96]",
        "stp x14, x15, [sp, #112]",
        "stp x16, x17, [sp, #128]",
        "stp x18, x19, [sp, #144]",
        "stp x20, x21, [sp, #160]",
        "stp x22, x23, [sp, #176]",
        "stp x24, x25, [sp, #192]",
        "stp x26, x27, [sp, #208]",
        "stp x28, x29, [sp, #224]",
        "str x30, [sp, #240]",
        "stp q0, q1, [sp, #256]",
        "stp q2, q3, [sp, #288]",
        "stp q4, q5, [sp, #320]",
        "stp q6, q7, [sp, #352]",
        "stp q8, q9, [sp, #384]",
        "stp q10, q11, [sp, #416]",
        "stp q12, q13, [sp, #448]",
        "stp q14, q15, [sp, #480]",
        "stp q16, q17, [sp, #512]",
        "stp q18, q19, [sp, #544]",
        "stp q20, q21, [sp, #576]",
        "stp q22, q23, [sp, #608]",
        "stp q24, q25, [sp, #640]",
        "stp q26, q27, [sp, #672]",
        "stp q28, q29, [sp, #704]",
        "stp q30, q31, [sp, #736]",
        "mov x0, sp",
        "bl {hit}",
        "ldp q0, q1, [sp, #256]",
        "ldp q2, q3, [sp, #288]",
        "ldp q4, q5, [sp, #320]",
        "ldp q6, q7, [sp, #352]",
        "ldp q8, q9, [sp, #384]",
        "ldp q10, q11, [sp, #416]",
        "ldp q12, q13, [sp, #448]",
        "ldp q14, q15, [sp, #480]",
        "ldp q16, q17, [sp, #512]",
        "ldp q18, q19, [sp, #544]",
        "ldp q20, q21, [sp, #576]",
        "ldp q22, q23, [sp, #608]",
        "ldp q24, q25, [sp, #640]",
        "ldp q26, q27, [sp, #672]",
        "ldp q28, q29, [sp, #704]",
        "ldp q30, q31, [sp, #736]",
        "ldp x0, x1, [sp]",
        "ldp x2, x3, [sp, #16]",
        "ldp x4, x5, [sp, #32]",
        "ldp x6, x7, [sp, #48]",
        "ldp x8, x9, [sp, #64]",
        "ldp x10, x11, [sp, #80]",
        "ldp x12, x13, [sp, #96]",
        "ldp x14, x15, [sp, #112]",
        "ldp x16, x17, [sp, #128]",
        "ldp x18, x19, [sp, #144]",
        "ldp x20, x21, [sp, #160]",
        "ldp x22, x23, [sp, #176]",
        "ldp x24, x25, [sp, #192]",
        "ldp x26, x27, [sp, #208]",
        "ldp x28, x29, [sp, #224]",
        "ldr x30, [sp, #240]",
        "add sp, sp, #768",
        "ret",
        hit = sym hit,
    )
}
