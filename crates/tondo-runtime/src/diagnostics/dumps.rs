//! Dumps (docs/refactor.md, phase 6): what a program writes of itself when it
//! fails, when a tool asks, at `kill -QUIT`, or when `TONDO_DUMP_WHEN` says,
//! and the command that takes each one away.

use super::*;

/// The contexts of a dump's task with a field a `with` set, with those
/// fields.
pub(super) fn contexts_json(described: Vec<crate::contexts::Described>) -> String {
    let contexts: Vec<String> = described
        .into_iter()
        .filter_map(|context| {
            let fields: Vec<String> = context
                .fields
                .iter()
                .filter(|field| field.set.is_some())
                .map(|field| format!("{}:{}", quoted(&field.name), quoted(&field.value)))
                .collect();
            (!fields.is_empty())
                .then(|| format!("{{\"name\":{},\"fields\":{{{}}}}}", quoted(&context.name), fields.join(",")))
        })
        .collect();
    format!("[{}]", contexts.join(","))
}

/// Whether a dump holds values: not with `TONDO_DUMP_VALUES=off`. A value
/// it does not know also turns them off, as a mistake must not show them.
pub(crate) fn values_shown() -> bool {
    config().dump_values.as_ref().is_none_or(|value| value == "on")
}

/// A dump of the program, as JSON: its counters, every task and where it
/// is, and the samples of its live memory. The workers stop for it a while
/// when `wait` says, so that the running tasks show where they are; `here`
/// is the stack of this thread's own task.
pub(super) fn dump(reason: &str, message: &str, here: Option<Vec<usize>>, wait: Option<std::time::Duration>) -> String {
    let (stopped, tasks, contexts, values) = crate::cycles::without_collections(|| {
        let stopped = wait.is_some_and(crate::tasks::stop_world);
        let current = here.is_some();
        let tasks = crate::tasks::snapshot(here);
        // The bindings of the frames, while nothing moves.
        let shown = values_shown();
        let values = if shown { crate::debugger::values(current.then(here_now)) } else { Vec::new() };
        // Each task's contexts (language 11.16), the fields a `with` set:
        // not at a fault, where showing them could not allocate.
        let contexts: Vec<String> = tasks
            .iter()
            .map(|task| match crate::tasks::by_id(task.id).filter(|_| shown && reason != "fault") {
                Some(pointer) => contexts_json(unsafe { crate::contexts::described(pointer) }),
                None => "[]".to_string(),
            })
            .collect();
        if stopped {
            crate::tasks::resume_world();
        }
        (stopped, tasks, contexts, values)
    });
    let (samples, dropped) = crate::heap::alive();
    let mut frames = Frames::default();
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let mut out = format!(
        "{{\"dump\":1,\"reason\":{},\"message\":{},\"info\":{},\"time_ms\":{time},\"stopped\":{stopped},\"counters\":{},\"tasks\":[",
        quoted(reason),
        quoted(message),
        info(),
        counter_list()
    );
    for (index, task) in tasks.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let locks: Vec<String> = task.locks.iter().map(|l| quoted(l)).collect();
        let serving = task.request.as_ref().filter(|_| values_shown());
        let serving = serving.map_or(String::new(), |request| format!(",\"request\":{}", quoted(request)));
        let wait = task.wait.as_ref().map_or(serving.clone(), |(text, on, object, ms)| {
            let on: Vec<String> = on.iter().map(u64::to_string).collect();
            format!(
                "{serving},\"wait\":{{\"for\":{},\"on\":[{}],\"object\":\"{object:#x}\",\"ms\":{ms}}}",
                quoted(text),
                on.join(",")
            )
        });
        let _ = write!(
            out,
            "{comma}{{\"id\":{},\"main\":{},\"state\":\"{}\",\"cancelled\":{},\"stack\":{},\"spawned\":{},\"locks\":[{}],\"contexts\":{}{wait}}}",
            task.id,
            task.main,
            task.state,
            task.cancelled,
            frames.list(&task.stack),
            frames.list(&task.spawned),
            locks.join(","),
            contexts[index]
        );
    }
    let _ = write!(out, "],\"heap\":{{\"rate\":{},\"dropped\":{dropped},\"samples\":[", crate::heap::rate());
    for (index, (size, stack)) in samples.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        let _ = write!(out, "{comma}{{\"size\":{size},\"stack\":{}}}", frames.list(stack));
    }
    out.push_str("]}");
    // A debug build knows every object, and the dump who holds each.
    if let Some(objects) = crate::heap::objects() {
        out.push_str(",\"objects\":{\"list\":[");
        for (index, (address, size, site, text)) in objects.list.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let text = text.as_ref().map_or(String::new(), |t| format!(",\"text\":{}", quoted(t)));
            let _ = write!(
                out,
                "{comma}{{\"address\":\"{address:#x}\",\"size\":{size},\"site\":{}{text}}}",
                frames.list(&[*site])
            );
        }
        let edges: Vec<String> = objects.edges.iter().map(|(from, to)| format!("[{from},{to}]")).collect();
        let roots: Vec<String> = objects.roots.iter().map(|(task, to)| format!("[{task},{to}]")).collect();
        let _ = write!(out, "],\"edges\":[{}],\"roots\":[{}]}}", edges.join(","), roots.join(","));
    }
    match values_shown() {
        true => {
            let flights = flights_json(&mut frames);
            out.push_str(&flights);
            let _ = write!(out, ",\"events\":{}", crate::events::json(|address| frames.list(&[address])));
        }
        false => out.push_str(",\"values_off\":true"),
    }
    let _ = write!(out, ",\"sources\":{}", quoted(&roots()));
    let orders: Vec<String> = crate::lockorder::found().iter().map(|text| quoted(text)).collect();
    let _ = write!(out, ",\"lock_order\":[{}]", orders.join(","));
    let _ = write!(out, ",\"values\":[{}]", values.join(","));
    let _ = write!(out, ",\"frames\":{{{}}}}}", frames.json);
    out
}

/// Where a crash stopped the program: the code of a fault, its frame and its
/// stack.
pub(super) static FAULTED: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

pub(crate) fn faulted_at(pc: usize, fp: usize, (low, high): (usize, usize)) {
    for (slot, value) in FAULTED.iter().zip([pc, fp, low, high]) {
        slot.store(value, Ordering::SeqCst);
    }
}

/// The registers at a fault, by number.
pub(super) static FAULT_REGISTERS: [AtomicUsize; 32] = [const { AtomicUsize::new(0) }; 32];

/// The fault stopped the program's code, and they are that code's: in C
/// they are C's.
pub(super) static FAULT_IN_CODE: AtomicBool = AtomicBool::new(false);

pub(crate) fn fault_registers(registers: &[usize; 32], in_code: bool) {
    for (slot, &value) in FAULT_REGISTERS.iter().zip(registers) {
        slot.store(value, Ordering::SeqCst);
    }
    FAULT_IN_CODE.store(in_code, Ordering::SeqCst);
}

pub(crate) fn faulted_registers() -> [Option<usize>; 32] {
    let known = FAULT_IN_CODE.load(Ordering::SeqCst);
    FAULT_REGISTERS.each_ref().map(|slot| known.then(|| slot.load(Ordering::SeqCst)))
}

/// Where the task of this thread stands: at the fault that stopped it, or
/// here. The code of a fault, its frame and its stack.
pub(super) fn here_now() -> (Option<usize>, usize, (usize, usize)) {
    match FAULTED.each_ref().map(|slot| slot.load(Ordering::SeqCst)) {
        [pc, fp, low, high] if fp != 0 => ((pc != 0).then_some(pc), fp, (low, high)),
        _ => (None, crate::trace::frame_pointer(), crate::tasks::stack_bounds().unwrap_or((0, 0))),
    }
}

/// `TONDO_DUMP` as the program was started with: the directory it asked its
/// dumps in, which also has it record what a dump of a running program
/// shows; none with `off`.
pub(crate) fn asked_dump() -> Option<std::ffi::OsString> {
    config().dump.clone().filter(|dir| dir != "off" && dir != "0")
}

/// How many dumps of crashes the cache keeps, the latest.
pub(super) const KEPT_DUMPS: usize = 20;

/// Where a crash leaves its dump: where `TONDO_DUMP` says, or, unless it is
/// `off`, in the user's cache of Tondo, which only the user can read, as a
/// dump holds the program's values; and whether it is that cache.
pub(super) fn crash_dir() -> Option<(PathBuf, bool)> {
    if let Some(dir) = asked_dump() {
        return Some((PathBuf::from(dir), false));
    }
    if config().dump.is_some() {
        return None;
    }
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("tondo").join("dumps");
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    let private = std::fs::symlink_metadata(&dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0);
    private.then_some((dir, true))
}

/// The latest dumps of the cache, and no more.
pub(super) fn prune_dumps(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dumps: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".dump.json"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    dumps.sort();
    let extra = dumps.len().saturating_sub(KEPT_DUMPS);
    for (_, path) in dumps.into_iter().take(extra) {
        let _ = std::fs::remove_file(path);
    }
}

/// A program that panics, faults or deadlocks leaves a dump before it ends,
/// named after it and its process: where `TONDO_DUMP` says, or in the cache.
pub(crate) fn crash_dump(reason: &str, message: &str, here: Vec<usize>, stop: bool) {
    if crate::debugger::rendering() {
        return;
    }
    let Some((dir, cache)) = crash_dir() else { return };
    let program = std::env::current_exe().ok().and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()));
    let name = format!("{}-{}.dump.json", program.unwrap_or_else(|| "tondo".into()), std::process::id());
    let path = dir.join(name);
    // This thread stands still too while the others stop.
    let paused = crate::tasks::pause_here();
    let text = dump(reason, message, Some(here), stop.then_some(std::time::Duration::from_millis(100)));
    if paused {
        crate::tasks::enter_tondo();
    }
    let _ = match std::fs::write(&path, text) {
        Ok(()) => writeln!(std::io::stderr(), "tondo runtime: a dump of the program is in {}", path.display()),
        Err(e) => writeln!(std::io::stderr(), "tondo runtime: cannot write a dump to {}: {e}", path.display()),
    };
    if cache {
        prune_dumps(&dir);
    }
    dumped(&path);
}

/// A dump at a logpoint, of the program as it stands where this thread is:
/// at the site `at`, its frame and its stack, with every register known.
pub(crate) fn dump_here(
    here: Vec<usize>,
    at: (usize, usize, (usize, usize)),
    registers: &[usize; 32],
    floats: &[usize; 32],
) -> String {
    let paused = crate::tasks::pause_here();
    let (pc, fp, bounds) = at;
    faulted_at(pc, fp, bounds);
    crate::debugger::site_registers(registers, floats, true);
    let text = dump("logpoint", "a logpoint was reached", Some(here), Some(std::time::Duration::from_millis(100)));
    crate::debugger::site_registers(registers, floats, false);
    faulted_at(0, 0, (0, 0));
    if paused {
        crate::tasks::enter_tondo();
    }
    text
}

/// How long `TONDO_DUMP_COMMAND` has: a program that falls waits for it, or
/// a restart would take the dump with its container.
pub(super) const COMMAND_TIME: std::time::Duration = std::time::Duration::from_secs(60);

/// `TONDO_DUMP_COMMAND`: a shell command that takes a dump just written
/// elsewhere, with its path in `TONDO_DUMP_FILE`.
pub(super) fn dumped(path: &std::path::Path) {
    let Some(command) = &config().dump_command else { return };
    let _outside = crate::replay::Outside::enter();
    let started = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .env("TONDO_DUMP_FILE", path)
        .env_remove("TONDO_DUMP")
        .env_remove("TONDO_DUMP_COMMAND")
        .stdin(std::process::Stdio::null())
        .spawn();
    let mut child = match started {
        Ok(child) => child,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "tondo runtime: cannot run TONDO_DUMP_COMMAND: {e}");
            return;
        }
    };
    let deadline = Instant::now() + COMMAND_TIME;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return,
            Ok(Some(status)) => {
                let _ = writeln!(std::io::stderr(), "tondo runtime: TONDO_DUMP_COMMAND ended with {status}");
                return;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ =
                    writeln!(std::io::stderr(), "tondo runtime: TONDO_DUMP_COMMAND took too long, and it is stopped");
                return;
            }
        }
    }
}

/// `TONDO_DUMP_WHEN`: dumps by themselves in `TONDO_DUMP`: `every=5m`, or
/// past `memory=512m` resident or `cpu=90` percent of a core, with no more
/// than one a minute of those, keeping the `keep=10` last. Parts go by
/// commas.
pub(super) struct Schedule {
    pub(super) every: Option<std::time::Duration>,
    pub(super) memory: Option<i64>,
    pub(super) cpu: Option<f64>,
    pub(super) keep: usize,
    /// How many bytes all of them may take.
    pub(super) room: Option<u64>,
    /// How long work may wait with nothing moving, and a request take.
    pub(super) stall: Option<std::time::Duration>,
    pub(super) request: Option<std::time::Duration>,
}

impl Schedule {
    pub(super) fn parse(text: &str) -> Result<Schedule, String> {
        let mut schedule =
            Schedule { every: None, memory: None, cpu: None, keep: 10, room: None, stall: None, request: None };
        for part in text.split(',').map(str::trim) {
            let wrong = || {
                format!(
                    "TONDO_DUMP_WHEN: `{part}` is not every=5m, memory=512m, memory=90%, cpu=90, stall=30s, request=5s, keep=10 or size=1g"
                )
            };
            let (name, value) = part.split_once('=').ok_or_else(wrong)?;
            match name {
                "every" => schedule.every = Some(interval(value).ok_or_else(wrong)?),
                "memory" => schedule.memory = Some(memory_limit(value).ok_or_else(wrong)?),
                "cpu" => schedule.cpu = Some(value.parse().map_err(|_| wrong())?),
                "keep" => schedule.keep = value.parse().map_err(|_| wrong())?,
                "size" => schedule.room = Some(size(value).ok_or_else(wrong)? as u64),
                "stall" => schedule.stall = Some(interval(value).ok_or_else(wrong)?),
                "request" => schedule.request = Some(interval(value).ok_or_else(wrong)?),
                _ => return Err(wrong()),
            }
        }
        Ok(schedule)
    }
}

/// `30s`, `5m` or `2h`.
pub(super) fn interval(text: &str) -> Option<std::time::Duration> {
    let (count, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let seconds = match unit {
        "ms" => return Some(std::time::Duration::from_millis(count.parse().ok()?)),
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(count.parse::<u64>().ok()? * seconds))
}

/// `512m`, or `90%` of what the program may hold: the limit of its cgroup,
/// past which the kernel kills it without a dump, or the machine's memory.
pub(super) fn memory_limit(text: &str) -> Option<i64> {
    let Some(percent) = text.strip_suffix('%') else { return size(text) };
    let percent: f64 = percent.parse().ok().filter(|p| *p > 0.0 && *p <= 100.0)?;
    Some((available_memory() as f64 * percent / 100.0) as i64)
}

/// What the program may hold: the lowest of its cgroup's limits, from its
/// own up to the root, or all the machine has.
pub(super) fn available_memory() -> i64 {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) } as i64;
    let machine = pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as i64;
    let membership = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    cgroup_limit(&membership, std::path::Path::new("/sys/fs/cgroup")).map_or(machine, |limit| limit.min(machine))
}

/// The lowest limit of the cgroups `/proc/self/cgroup` names, under `root`.
pub(super) fn cgroup_limit(membership: &str, root: &std::path::Path) -> Option<i64> {
    let mut lowest = None;
    let mut look = |file: PathBuf| {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        if let Ok(limit) = text.trim().parse::<i64>() {
            lowest = Some(lowest.map_or(limit, |known: i64| known.min(limit)));
        }
    };
    for line in membership.lines() {
        let mut parts = line.splitn(3, ':');
        let (_, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
        let path = path.trim_start_matches('/');
        // Version 2 has a line of no controllers; version 1, one for memory.
        let (base, name) = match controllers {
            "" => (root.to_path_buf(), "memory.max"),
            list if list.split(',').any(|c| c == "memory") => (root.join("memory"), "memory.limit_in_bytes"),
            _ => continue,
        };
        let mut dir = base.join(path);
        loop {
            look(dir.join(name));
            if dir == base || !dir.pop() {
                break;
            }
        }
    }
    lowest
}

/// `512m` or `2g`.
pub(super) fn size(text: &str) -> Option<i64> {
    let (count, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let scale = match unit.to_ascii_lowercase().as_str() {
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        _ => return None,
    };
    Some(count.parse::<i64>().ok()? * scale)
}

/// The processor time the program used so far.
pub(super) fn processor_time() -> std::time::Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let of = |time: libc::timeval| std::time::Duration::new(time.tv_sec as u64, time.tv_usec as u32 * 1000);
    of(usage.ru_utime) + of(usage.ru_stime)
}

/// The stack of a thread that writes dumps, which it compresses.
pub(super) const DUMPING_STACK: usize = 512 * 1024;

/// A dump as a program that goes on writes it, which takes a tenth of the
/// room; one at a crash stays as it is, as little as possible runs there.
pub(super) fn compressed(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let written = encoder.write_all(text.as_bytes());
    written.and_then(|()| encoder.finish()).unwrap_or_default()
}

/// Writes a dump that a thread makes while the program goes on: aside, then
/// renamed, so that a program that ends meanwhile leaves no dump half
/// written, only, at worst, the hidden file it wrote aside.
fn write_whole(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let aside = path.with_file_name(format!(".{name}.part"));
    std::fs::write(&aside, bytes)?;
    std::fs::rename(&aside, path)
}

/// `kill -QUIT` without `TONDO_DUMP`: where each task is, for whoever asks a
/// program that seems stuck, with nothing set up beforehand.
pub(crate) fn tasks_to_stderr() {
    crate::flush_stdout();
    let tasks = crate::cycles::without_collections(|| {
        let stopped = crate::tasks::stop_world(std::time::Duration::from_millis(200));
        let tasks = crate::tasks::snapshot(None);
        if stopped {
            crate::tasks::resume_world();
        }
        tasks
    });
    let mut report = String::from("SIGQUIT: where the tasks are");
    for task in tasks {
        let main = if task.main { " (main)" } else { "" };
        let title = format!("task {}{main} {}", task.id, task.state);
        crate::trace::write_stack(&mut report, &title, || task.stack.iter().copied(), None);
    }
    let _ = writeln!(std::io::stderr(), "{report}");
}

/// `kill -QUIT`, with `TONDO_DUMP`: a dump of the program, which goes on.
pub(crate) fn dump_on_signal() {
    static NUMBER: AtomicU64 = AtomicU64::new(0);
    let Some(dir) = asked_dump() else { return };
    let number = NUMBER.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = crate::tasks::try_spawn_thread(Some(DUMPING_STACK), move || {
        retire_here();
        let program = std::env::current_exe().ok().and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()));
        let program = program.unwrap_or_else(|| "tondo".into());
        let path = PathBuf::from(dir).join(format!("{program}-{}-quit-{number}.dump.json.gz", std::process::id()));
        let text = dump("signal", "SIGQUIT", None, Some(std::time::Duration::from_millis(100)));
        if write_whole(&path, &compressed(&text)).is_ok() {
            let _ = writeln!(std::io::stderr(), "tondo runtime: a dump of the program is in {}", path.display());
            dumped(&path);
        }
    });
}

/// Starts the thread of `TONDO_DUMP_WHEN`, when it is set with `TONDO_DUMP`.
pub(super) fn schedule_dumps() {
    let (Some(when), Some(dir)) = (config().dump_when.clone(), asked_dump()) else {
        return;
    };
    let schedule = match Schedule::parse(&when) {
        Ok(schedule) => schedule,
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "tondo runtime: {message}");
            return;
        }
    };
    let _ =
        crate::tasks::try_spawn_thread(Some(DUMPING_STACK), move || dumps_on_schedule(schedule, PathBuf::from(dir)));
}

pub(super) fn dumps_on_schedule(schedule: Schedule, dir: PathBuf) {
    retire_here();
    let tick = std::time::Duration::from_secs(1).min(schedule.every.unwrap_or(std::time::Duration::MAX));
    let least = std::time::Duration::from_secs(60);
    let program = std::env::current_exe().ok().and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()));
    let program = program.unwrap_or_else(|| "tondo".into());
    let mut written: std::collections::VecDeque<(PathBuf, u64)> = Default::default();
    let (mut number, mut used, mut measured) = (0, processor_time(), Instant::now());
    let (mut last_timed, mut last_over) = (Instant::now(), None::<Instant>);
    let (mut turns, mut still_from, mut slow_seen) = (crate::tasks::turns(), Instant::now(), None::<u64>);
    loop {
        std::thread::sleep(tick);
        let now = Instant::now();
        // Nothing moved while there was work: tasks that wait for a worker,
        // or requests no task answers.
        let requests = flights_now();
        let turned = crate::tasks::turns();
        if turned != turns || (crate::tasks::ready_tasks() == 0 && requests.is_empty()) {
            (turns, still_from) = (turned, now);
        }
        let stalled = schedule.stall.filter(|&stall| now.duration_since(still_from) >= stall).map(|stall| {
            still_from = now;
            let (ready, flying) = (crate::tasks::ready_tasks(), requests.len());
            format!("nothing moved for {stall:?}, with {ready} tasks ready and {flying} requests in flight")
        });
        // A request past its limit, once each.
        let slow = schedule.request.and_then(|limit| {
            let past = |(id, ..): &&(u64, String, std::time::Duration)| slow_seen.is_none_or(|seen| *id > seen);
            let found = requests.iter().filter(past).find(|(.., took)| *took >= limit)?;
            slow_seen = Some(found.0);
            Some(format!("{} took {:?}", found.1, found.2))
        });
        let spent = processor_time();
        let busy = (spent - used).as_secs_f64() / now.duration_since(measured).as_secs_f64().max(0.001) * 100.0;
        (used, measured) = (spent, now);
        let over = match (schedule.memory, schedule.cpu) {
            (Some(most), _) if resident() > most => Some(format!("{} bytes resident, over {most}", resident())),
            (_, Some(most)) if busy > most => Some(format!("{busy:.0}% of a core, over {most}%")),
            _ => None,
        };
        let over = over.filter(|_| last_over.is_none_or(|at| now.duration_since(at) >= least));
        let over = stalled.or(slow).or(over);
        let timed =
            schedule.every.filter(|&every| now.duration_since(last_timed) >= every).map(|_| "on time".to_string());
        let Some(why) = over.clone().or(timed) else { continue };
        if over.is_some() {
            last_over = Some(now);
        } else {
            last_timed = now;
        }
        number += 1;
        let path = dir.join(format!("{program}-{}-{number}.dump.json.gz", std::process::id()));
        let text = compressed(&dump("schedule", &why, None, Some(std::time::Duration::from_millis(100))));
        let length = text.len() as u64;
        let forget = |written: &mut std::collections::VecDeque<(PathBuf, u64)>| {
            if let Some((oldest, _)) = written.pop_front() {
                let _ = std::fs::remove_file(oldest);
            }
        };
        // The room holds before a dump is written. What it keeps goes only
        // once the new one is there: the program may end at any moment, and
        // the last ones are what is wanted of a program that ended.
        let room = schedule.room.unwrap_or(u64::MAX);
        while !written.is_empty() && written.iter().map(|(_, size)| size).sum::<u64>() + length > room {
            forget(&mut written);
        }
        if write_whole(&path, &text).is_ok() {
            written.push_back((path.clone(), length));
            while written.len() > schedule.keep.max(1) {
                forget(&mut written);
            }
            dumped(&path);
        }
    }
}

/// Writes a dump of the program to `path`, as a debugger asks with the
/// program stopped, gdb's or lldb's: whether it could. Its threads stand
/// still, so it waits for none of them. The debugger calls it on a thread
/// it stopped, whose task stopped at `pc` with the frame pointer `fp`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_dump_to(path: *const std::ffi::c_char, pc: usize, fp: usize) -> i32 {
    if path.is_null() {
        return 0;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().into_owned();
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let here = std::iter::once(pc).chain(crate::trace::walk(fp, bounds)).collect();
    let text = dump("debugger", "asked by a debugger", Some(here), None);
    std::fs::write(path, text).is_ok() as i32
}

/// The sets of labels a document's samples refer to by number, each
/// described once: `"number": [["name", "value"], ...]`.
#[derive(Default)]
pub(crate) struct Labels {
    pub(super) described: std::collections::HashSet<u32>,
    pub(crate) json: String,
}

impl Labels {
    /// The number of a sample's set, describing it if it is new; `0`, for
    /// none or for a set no longer kept.
    pub(crate) fn number(&mut self, set: u32) -> u32 {
        let Some(labels) = crate::labels::get(set) else { return 0 };
        if self.described.insert(set) {
            let pairs: Vec<String> =
                labels.iter().map(|(name, value)| format!("[{},{}]", quoted(name), quoted(value))).collect();
            let comma = if self.json.is_empty() { "" } else { "," };
            let _ = write!(self.json, "{comma}\"{set}\":[{}]", pairs.join(","));
        }
        set
    }
}

/// The frames a document refers to by address, each described once.
#[derive(Default)]
pub(crate) struct Frames {
    pub(super) described: std::collections::HashSet<usize>,
    pub(crate) json: String,
}

impl Frames {
    /// The addresses as a JSON array, describing the new ones.
    pub(crate) fn list(&mut self, addresses: &[usize]) -> String {
        let mut out = String::from("[");
        for (index, &address) in addresses.iter().enumerate() {
            let comma = if index == 0 { "" } else { "," };
            let _ = write!(out, "{comma}\"{address:x}\"");
            if self.described.insert(address) {
                let comma = if self.json.is_empty() { "" } else { "," };
                let _ = write!(self.json, "{comma}\"{address:x}\":{}", frame(address));
            }
        }
        out.push(']');
        out
    }
}

pub(crate) fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
