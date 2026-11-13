//! Stopping (docs/refactor.md, phase 6): breakpoints, steps and the events a
//! debugger asks to stop at, where a task stops, and every other task with it
//! at its next probe, until they all go on together.

use super::*;

/// How the debugger asked the program to go on: until a breakpoint, or for
/// one task, to the next line, the next one in its frame or an outer one,
/// or the first in an outer frame.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Run,
    /// One line on, in `frame` or in what it calls.
    Step {
        task: u64,
        frame: usize,
    },
    Next {
        task: u64,
        frame: usize,
    },
    Finish {
        task: u64,
        frame: usize,
    },
    /// A frame restarts: stop where its function's new call starts.
    Restarted {
        task: u64,
    },
    /// Into the call of a function of the name `INTO` holds that the frame
    /// makes, or on to its next line.
    Into {
        task: u64,
        frame: usize,
    },
}

/// The name `step into` goes into.
pub(super) static INTO: Mutex<String> = Mutex::new(String::new());

/// Whether a function is called `name`: by its whole name, as
/// `Stack.push`, or by its own, `push`, with a generic one's type
/// arguments or without; `loose`, also by the name after the last dot, as
/// a call `items.push` names `Array.push`.
pub(super) fn named(function: &Function, name: &str, loose: bool) -> bool {
    let mut plain = String::new();
    let mut depth = 0;
    for c in function.name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let own = name.rsplit('.').next().unwrap_or(name);
    let by_own = loose || own == name;
    function.name == name || plain == name || (by_own && (plain == own || plain.ends_with(&format!(".{own}"))))
}

/// Where the task that stopped is: its probe, the frame of its function
/// and its stack's bounds, and its bindings.
#[derive(Clone)]
pub(super) struct Stop {
    pub(super) task: u64,
    pub(super) site: usize,
    pub(super) frame: usize,
    pub(super) bounds: (usize, usize),
    pub(super) addresses: usize,
    pub(super) alive: usize,
    pub(super) why: String,
}

pub(super) struct State {
    pub(super) mode: Mode,
    /// Where the code of each stopped task is at, when known.
    pub(super) code: Vec<(u64, usize)>,
    pub(super) stopped: Option<Stop>,
    /// Counts the times the program went on; a stopped task waits for it to
    /// change.
    pub(super) resumed: u64,
    /// Where each held task is, and its frame.
    pub(super) held: Vec<Stop>,
    /// The task and frame whose bindings `locals` and `print` show; the
    /// stopped task's own by default.
    pub(super) selected: Option<(u64, usize)>,
    pub(super) breakpoints: Vec<Breakpoint>,
    pub(super) watches: Vec<Watch>,
    pub(super) next_breakpoint: u32,
    /// The moment of the history shown instead of the present.
    pub(super) past: Option<usize>,
    /// The tasks that stay held while the program goes on.
    pub(super) frozen: Vec<u64>,
}

/// Some task is frozen: probes look.
pub(super) static FREEZING: AtomicBool = AtomicBool::new(false);

/// An expression a task's frame watches: the task stops on the line after
/// the one that changed its value, in that frame or a call it makes, until
/// the frame returns. One of a `std.sync` cell's value watches it from
/// every task, for as long as the watch lasts.
pub(super) struct Watch {
    pub(super) id: u32,
    pub(super) task: u64,
    pub(super) expr: Expr,
    pub(super) text: String,
    /// The frame, and those that called it, which live while it does.
    pub(super) frames: Vec<Frame>,
    /// The value as it was, and the frame and site of the task's last
    /// probe in each frame from the watched one in, the outer first.
    pub(super) value: String,
    pub(super) last: Vec<(usize, usize)>,
    /// When a change stops: its condition holds, and its count of such
    /// changes reaches `hits`.
    pub(super) condition: Option<Expr>,
    pub(super) count: u64,
    pub(super) hits: Option<Hits>,
    /// The cell, which the watch keeps alive, the binding it was read from
    /// and how that shows it.
    pub(super) shared: Option<(usize, String, Show)>,
}

impl Watch {
    /// Its value now, and whether its condition holds; `None` when it
    /// cannot be read now, as while a task changes a cell.
    pub(super) fn now(&self, table: &Table) -> Option<(String, bool)> {
        let evaluate = |e: &mut Evaluator| {
            let value = e.evaluate(&self.expr).ok()?.text;
            Some((value, self.condition.as_ref().is_none_or(|c| e.holds(c))))
        };
        match &self.shared {
            None => evaluating(table, &self.frames, 0, evaluate),
            Some((cell, root, glue)) => {
                let read = |name: &str| match name == root {
                    true => Ok(show(*glue, cell as *const usize as usize)),
                    false => Err(format!("a watch of a shared value reads `{root}` only")),
                };
                unsafe {
                    crate::sync::reading(*cell as *mut crate::sync::SyncObject, || evaluate(&mut Evaluator::new(&read)))
                }
                .flatten()
            }
        }
    }
}

/// Removes the watches `unwanted` says, letting go of the cells they kept.
pub(super) fn unwatch(state: &mut State, unwanted: impl Fn(&Watch) -> bool) {
    state.watches.retain(|watch| {
        let keep = !unwanted(watch);
        if let Some((cell, ..)) = watch.shared.as_ref().filter(|_| !keep) {
            unsafe { crate::sync::tondo_rt_sync_release(*cell as *mut crate::sync::SyncObject) };
        }
        keep
    });
    WATCHING.store(!state.watches.is_empty(), Ordering::Relaxed);
}

pub(super) struct Breakpoint {
    pub(super) id: u32,
    pub(super) sites: Vec<usize>,
    pub(super) condition: Option<Expr>,
    /// How many times its condition held here, and from which count on it
    /// stops; and the message it writes instead of stopping (a logpoint).
    pub(super) count: u64,
    pub(super) hits: Option<Hits>,
    pub(super) log: Option<String>,
    /// The task it stops alone, when it names one, and the request it stops
    /// in alone: a text its method and path have.
    pub(super) task: Option<u64>,
    pub(super) request: Option<String>,
    /// `until`'s: it goes at the next stop, wherever that is.
    pub(super) once: bool,
    /// The event it stops at, instead of a place.
    pub(super) event: Option<Event>,
    /// It captures the stack and the bindings and goes on, instead of
    /// stopping (`capture`).
    pub(super) capture: bool,
    /// Its condition, when only a compiled patch reads it.
    pub(super) unparsed: Option<String>,
    /// Its condition compiled by live patches: for each, the code, the
    /// bindings it reads, by name, in order, and the sites it is for, those
    /// of the code it was compiled with, whose types it knows.
    pub(super) compiled: Vec<(u64, Vec<String>, Vec<usize>)>,
}

/// Whether a compiled condition holds in a frame: its code called with the
/// addresses of the bindings it reads, on this task, its lines passing.
/// None when one of them is not there yet, for the evaluator to say.
pub(super) fn compiled_holds(table: &Table, frames: &[Frame], code: u64, names: &[String]) -> Option<bool> {
    let bound = visible(table, frames, 0);
    let addresses: Option<Vec<u64>> =
        names.iter().map(|name| bound.iter().find(|(b, ..)| b.name == *name).map(|(_, a, _)| *a as u64)).collect();
    let addresses = addresses?;
    let condition: extern "C" fn(u64, u64) -> u8 = unsafe { std::mem::transmute(code as usize) };
    let _outside = crate::replay::Outside::enter();
    let before = SHOWING.replace(true);
    let held = condition(addresses.as_ptr() as u64, 0) != 0;
    SHOWING.set(before);
    Some(held)
}

/// When a breakpoint stops by its count of hits: at the `n`th, from it on,
/// past it, or at every `n`th.
pub(crate) struct Hits {
    pub(super) op: &'static str,
    pub(super) n: u64,
}

impl Hits {
    pub(crate) fn parse(text: &str) -> Result<Hits, String> {
        let text = text.trim();
        for op in [">=", ">", "==", "%"] {
            if let Some(n) = text.strip_prefix(op).and_then(|n| n.trim().parse().ok()) {
                return Ok(Hits { op, n });
            }
        }
        match text.parse() {
            Ok(n) => Ok(Hits { op: "==", n }),
            Err(_) => Err(format!("a count of hits is `n`, `== n`, `>= n`, `> n` or `% n`, not `{text}`")),
        }
    }

    pub(crate) fn reached(&self, count: u64) -> bool {
        match self.op {
            ">=" => count >= self.n,
            ">" => count > self.n,
            "%" => self.n > 0 && count.is_multiple_of(self.n),
            _ => count == self.n,
        }
    }
}

/// A logpoint's message: each `{expression}` in it replaced by its value
/// there, a text without its quotes.
pub(super) fn logged(table: &Table, frames: &[Frame], message: &str) -> String {
    let mut out = String::new();
    for (text, expr) in pieces(message) {
        match expr.map(|expr| evaluating(table, frames, 0, |e| e.evaluate(&expr?).map(|v| v.value.written()))) {
            None => out.push_str(text),
            Some(Ok(value)) => out.push_str(&value),
            Some(Err(_)) => out.push_str(&format!("{{{text}?}}")),
        }
    }
    out
}

/// A logpoint's message in pieces: a text, or an expression in braces with
/// its text.
pub(super) fn pieces(message: &str) -> Vec<(&str, Option<Result<Expr, String>>)> {
    let mut out = Vec::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        out.push((&rest[..open], None));
        let text = rest[open + 1..open + close].trim();
        out.push((text, Some(expression::parse(text))));
        rest = &rest[open + close + 1..];
    }
    out.push((rest, None));
    out
}

pub(super) static STATE: Mutex<State> = Mutex::new(State {
    code: Vec::new(),
    mode: Mode::Run,
    stopped: None,
    resumed: 0,
    held: Vec::new(),
    selected: None,
    breakpoints: Vec::new(),
    watches: Vec::new(),
    next_breakpoint: 1,
    past: None,
    frozen: Vec::new(),
});
pub(super) static RESUMED: Condvar = Condvar::new();
/// The tool that debugs the program, which hears when it stops, and the
/// descriptor of its connection to the diagnostics thread.
pub(super) static CLIENT: Mutex<Option<UnixStream>> = Mutex::new(None);
pub(super) static CLIENT_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
/// The program started under `tondo debug`, which it waits for before
/// `main`; and it has not reached `main` yet.
pub(crate) static WAIT: AtomicBool = AtomicBool::new(false);

/// The debugger debugs the children the program starts too (`follow`).
pub(super) static FOLLOW: AtomicBool = AtomicBool::new(false);

/// Whether a child the program starts is for the debugger too: one that is
/// a debug build of Tondo waits for it before its `main`, as `tondo debug`
/// starts a program.
pub(crate) fn following() -> bool {
    follows_children() && !crate::replay::replaying()
}

/// The debugger asked to debug the children the program starts too.
pub(crate) fn follows_children() -> bool {
    attached() && FOLLOW.load(Ordering::Relaxed)
}

/// What gdb or lldb asks of the frame they show, of the program's function
/// at `pc` whose frame pointer is `fp`: `locals`, or `print <expression>`,
/// written to `path` as this debugger answers. The debugger calls it on
/// the thread it stopped; whether it could.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_to(
    path: *const std::ffi::c_char,
    command: *const std::ffi::c_char,
    pc: usize,
    fp: usize,
) -> i32 {
    let (Some(table), false, false) = (table(), path.is_null(), command.is_null()) else { return 0 };
    let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().into_owned();
    let command = unsafe { std::ffi::CStr::from_ptr(command) }.to_string_lossy().into_owned();
    std::fs::write(path, native_frame(&table, command.trim(), pc, fp)).is_ok() as i32
}

/// The answer to `command` in the frame of another debugger.
pub(super) fn native_frame(table: &Table, command: &str, pc: usize, fp: usize) -> String {
    let error = |message: &str| format!("{{\"error\":{}}}", quoted(message));
    let Some(code) = crate::trace::describe(pc + 1) else { return error("this frame is not the program's code") };
    let Some(function) = table.functions.iter().position(|f| f.name == code.function) else {
        return error(&format!("`{}` is not a function of the program", code.function));
    };
    let offsets = table.offsets.get(function).map(|(a, l)| (a.load(Ordering::Relaxed), l.load(Ordering::Relaxed)));
    let Some((addressed, living)) = offsets.filter(|&(a, _)| a != UNKNOWN) else {
        return error("the function has reached none of its lines yet: step to one");
    };
    let place = code.at.map_or(String::new(), |(file, line)| format!("{file}:{line} in {}", code.function));
    let at = |offset: isize| (fp as isize + offset) as usize;
    let frames =
        [Frame { place, function: Some(function), pointer: fp, addresses: at(addressed), alive: at(living), pc }];
    let (word, rest) = command.split_once(' ').unwrap_or((command, ""));
    match word {
        "locals" => format!("{{\"locals\":[{}]}}", locals_of(table, &frames, 0, crate::tasks::current_id()).join(",")),
        "print" => match expression::parse(rest).and_then(|e| evaluating(table, &frames, 0, |ev| ev.evaluate(&e))) {
            Ok(evaluated) => format!("{{\"name\":{},{}}}", quoted(rest), described(&evaluated.text)),
            Err(message) => error(&message),
        },
        _ => error("gdb and lldb ask `locals` or `print <expression>`"),
    }
}

/// The program started a child the debugger follows, which a replay
/// replays from its own `recording`.
pub(crate) fn child_started(pid: u32, program: &str, recording: Option<&std::path::Path>) {
    let recording = recording.map_or(String::new(), |r| format!(",\"recording\":{}", quoted(&r.to_string_lossy())));
    tell(&format!("{{\"child\":{{\"pid\":{pid},\"program\":{}{recording}}}}}", quoted(program)));
}
pub(super) static BEFORE_MAIN: AtomicBool = AtomicBool::new(true);

pub(super) fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn tell(event: &str) {
    let _outside = crate::replay::Outside::enter();
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(client, "{event}");
    }
}

/// The probe where a line starts: stops the task when the debugger asked
/// for it here. In a live build, whether the frame is to restart: the
/// statement after the probe then returns and calls its function again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_line(site: i64, addresses: *const usize, alive: *const u64) -> u8 {
    // The probes the program passes place a replay's calls: not those of
    // the code the debugger runs, to show a value or make a call.
    if crate::replay::on() && !SHOWING.get() && CALLING.load(Ordering::Relaxed) != crate::tasks::current_id() {
        crate::replay::tick();
    }
    // A reload that converts values with identity stops every task at a
    // statement first.
    if crate::live::barrier() {
        crate::live::park();
    }
    // A recording of the last minutes keeps a copy of the program now and
    // then, which a replay goes on from.
    if crate::replay::copy_due() {
        let frame = unsafe { *(crate::trace::frame_pointer() as *const usize) };
        if let Some(stop) = copied(site as usize, frame, addresses as usize, alive as usize) {
            hold(state(), stop, Some(CHECKPOINT.into()));
        }
    }
    if VALUES_ON.load(Ordering::Relaxed) {
        let frame = unsafe { *(crate::trace::frame_pointer() as *const usize) };
        line_values(site as usize, frame, addresses as usize, alive as usize);
    }
    if crate::live::holds_panics() {
        let frame = unsafe { *(crate::trace::frame_pointer() as *const usize) };
        let (site, addresses, alive) = (site as usize, addresses as usize, alive as usize);
        crate::tasks::note_probe(crate::tasks::Probe { frame, site, addresses, alive });
    }
    if !attached() {
        return crate::live::moving(site);
    }
    // A call the debugger makes runs to its end, as a view's code does.
    let calling = CALLING.load(Ordering::Relaxed);
    if (calling != 0 && calling == crate::tasks::current_id()) || SHOWING.get() {
        return 0;
    }
    let Some(mut table) = table() else { return 0 };
    let site = site as usize;
    crate::live::forget_goto(crate::tasks::current_id());
    // The frame of the function that has the probe.
    let frame = unsafe { *(crate::trace::frame_pointer() as *const usize) };
    let task = crate::tasks::current_id();
    // Where this function keeps its bindings, for the stacks it is in.
    if let Some((addressed, living)) = table.sites.get(site).and_then(|s| table.offsets.get(s.function))
        && addressed.load(Ordering::Relaxed) == UNKNOWN
    {
        addressed.store(addresses as isize - frame as isize, Ordering::Relaxed);
        living.store(alive as isize - frame as isize, Ordering::Relaxed);
    }
    let marked = table.marked.get(site).is_some_and(|m| m.load(Ordering::Relaxed));
    let watching = WATCHING.load(Ordering::Relaxed);
    let recording = RECORDING.load(Ordering::Relaxed) > 0;
    let waiting = STARTED.load(Ordering::Relaxed) || FREEZING.load(Ordering::Relaxed);
    let aiming = AIM.load(Ordering::Relaxed) != u64::MAX;
    // A replay that runs keeps a checkpoint now and then, looked at every
    // so many probes.
    let every = EVERY.load(Ordering::Relaxed);
    if every > 0 && crate::replay::look_at(&CHECKPOINT_LOOK, 1 << 14) {
        let _outside = crate::replay::Outside::enter();
        let here = Stop {
            task,
            site,
            frame,
            bounds: (0, 0),
            addresses: addresses as usize,
            alive: alive as usize,
            why: String::new(),
        };
        if let Some(stop) = periodic(&table, here, every) {
            let Some(again) = at_checkpoint(table, stop) else { return 0 };
            table = again;
        }
    }
    if !marked
        && !watching
        && !recording
        && !waiting
        && !aiming
        && !HOLD.load(Ordering::Relaxed)
        && state().mode == Mode::Run
    {
        return restarting(frame, task, site);
    }
    // What the debugger does here is not the program's, until the task
    // stops, when what the debugger asks it to run may be.
    let _outside = crate::replay::Outside::enter();
    let quiet = crate::heap::Quiet::enter();
    if recording {
        let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
        let (addresses, alive) = (addresses as usize, alive as usize);
        record(&table, &Stop { task, site, frame, bounds, addresses, alive, why: String::new() });
    }
    let mut state = state();
    // A replay going to a probe (`seek`), or through the run to it, keeping
    // where breakpoints would stop and where a task's lines start (`scan`).
    if aiming {
        let tick = crate::replay::ticks();
        let scanning = SCANNING.load(Ordering::Relaxed);
        // Breakpoints count their hits, as they would have, and stop
        // nowhere before the probe it goes to.
        scanned(&table, &mut state, task, site, frame, (addresses as usize, alive as usize), tick);
        let at = (addresses as usize, alive as usize);
        tracking(
            &table,
            Stop { task, site, frame, bounds: (0, 0), addresses: at.0, alive: at.1, why: String::new() },
            tick,
        );
        if AIM.load(Ordering::Relaxed) == tick {
            AIM.store(u64::MAX, Ordering::Relaxed);
            SCANNING.store(false, Ordering::Relaxed);
            drop(table);
            let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
            let (addresses, alive) = (addresses as usize, alive as usize);
            let stop = Stop { task, site, frame, bounds, addresses, alive, why: String::new() };
            drop(quiet);
            hold(state, stop, Some(if scanning { "scanned" } else { "replayed" }.to_string()));
            return restarting(frame, task, site);
        }
        return restarting(frame, task, site);
    }
    let frozen = state.frozen.contains(&task);
    // A task that started while a breakpoint stops tasks as they start, at
    // its first line of the program's code.
    let function = table.sites.get(site).map(|s| table.functions[s.function].name.as_str());
    let starts = STARTED.load(Ordering::Relaxed) && function.is_some_and(|f| !f.starts_with("spawn "));
    let started = starts.then(|| {
        let own = table.sites.get(site).is_some_and(|s| !s.generated());
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        let at = starting.iter().position(|(t, why)| *t == task && (why.is_none() || own));
        let found = at.map(|at| starting.swap_remove(at).1);
        STARTED.store(!starting.is_empty(), Ordering::Relaxed);
        found
    });
    let why = if frozen || state.stopped.is_some() || HOLD.load(Ordering::Relaxed) {
        // Another task stopped, or the debugger asked all to, or this task
        // stays where it is.
        if frozen || state.stopped.is_some() { None } else { Some("pause".to_string()) }
    } else if let Some(Some(told)) = started {
        let from = spawned(task).first().map_or(String::new(), |at| format!(", spawned at {at}"));
        Some(told.unwrap_or_else(|| format!("task {task} starts{from}")))
    } else {
        // A step goes over code the program's files do not have, the
        // standard library's and the derived, unless it started there.
        let standard = || table.sites.get(site).is_some_and(Site::generated);
        let goes_on = table.sites.get(site).is_some_and(|s| s.goes_on);
        let stepped = match state.mode {
            // The call asked for, made by the frame itself, or the frame's
            // next line.
            Mode::Into { task: t, frame: f } if t == task => {
                let function = table.sites.get(site).map(|s| &table.functions[s.function]);
                let caller = unsafe { *(frame as *const usize) };
                let into = INTO.lock().unwrap_or_else(|e| e.into_inner());
                (frame >= f && !standard()) || (caller == f && function.is_some_and(|f| named(f, &into, true)))
            }
            Mode::Into { .. } => false,
            Mode::Step { .. } | Mode::Next { .. } | Mode::Finish { .. }
                if !STEPS_STD.load(Ordering::Relaxed) && standard() =>
            {
                false
            }
            Mode::Run => false,
            // The line it stepped from going on is no new line.
            Mode::Step { task: t, frame: f } => t == task && !(frame == f && goes_on),
            Mode::Next { task: t, frame: f } => t == task && frame >= f && !(frame == f && goes_on),
            Mode::Finish { task: t, frame: f } => t == task && frame > f,
            // The first line of the call that restarted the frame.
            Mode::Restarted { task: t } => {
                let function = table.sites.get(site).map(|s| table.functions[s.function].name.as_str());
                t == task && function == Some(RESTARTED.lock().unwrap_or_else(|e| e.into_inner()).as_str())
            }
        };
        // A breakpoint whose condition holds counts the hit; it stops when
        // its count says so, or writes its message and goes on. Both read
        // what the code here can use, as `locals` shows it.
        let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
        let here = Stop {
            task,
            site,
            frame,
            bounds,
            addresses: addresses as usize,
            alive: alive as usize,
            why: String::new(),
        };
        let stack = std::cell::OnceCell::new();
        let stack = || stack.get_or_init(|| frames(&table, &here));
        let mut hit = None;
        for breakpoint in state.breakpoints.iter_mut() {
            let evaluated = || match &breakpoint.condition {
                Some(c) => evaluating(&table, stack(), 0, |e| e.holds(c)),
                None => breakpoint.unparsed.is_none(),
            };
            let holds = || match breakpoint.compiled.iter().find(|(.., sites)| sites.contains(&site)) {
                Some((code, names, _)) => compiled_holds(&table, stack(), *code, names).unwrap_or_else(evaluated),
                None => evaluated(),
            };
            let here = breakpoint.sites.contains(&site)
                && breakpoint.task.is_none_or(|t| t == task)
                && in_request(&breakpoint.request, task)
                && holds();
            if !here {
                continue;
            }
            breakpoint.count += 1;
            if !breakpoint.hits.as_ref().is_none_or(|hits| hits.reached(breakpoint.count)) {
                continue;
            }
            // A capture: the stack and the bindings, as the program goes on.
            if breakpoint.capture {
                let frames = stack();
                let locals = locals_of(&table, frames, 0, task).join(",");
                let places: Vec<String> = frames.iter().map(|f| quoted(&f.place)).collect();
                tell(&format!(
                    "{{\"captured\":{{\"id\":{},\"task\":{task},\"at\":{},\"locals\":[{locals}],\"stack\":[{}]}}}}",
                    breakpoint.id,
                    quoted(&place_in(&table, site)),
                    places.join(",")
                ));
                continue;
            }
            match &breakpoint.log {
                Some(message) => {
                    let text = logged(&table, stack(), message);
                    tell(&format!(
                        "{{\"logged\":{},\"task\":{task},\"at\":{}}}",
                        quoted(&text),
                        quoted(&place_in(&table, site))
                    ));
                }
                None => {
                    hit.get_or_insert(breakpoint.id);
                }
            }
        }
        let changed = if watching { watched(&table, &mut state, task, frame, site) } else { None };
        let until = hit.is_some_and(|id| state.breakpoints.iter().any(|b| b.id == id && b.once));
        match (hit, stepped) {
            (Some(_), _) if until => Some("until".to_string()),
            (Some(id), _) => Some(format!("breakpoint {id}")),
            _ if changed.is_some() => changed,
            (None, true) if matches!(state.mode, Mode::Restarted { .. }) => {
                RESTART_FRAME.store(0, Ordering::Relaxed);
                Some("restart".to_string())
            }
            (None, true) => Some("step".to_string()),
            (None, false) => return restarting(frame, task, site),
        }
    };
    // `until`'s breakpoint goes at the stop.
    if why.is_some() && state.breakpoints.iter().any(|b| b.once) {
        state.breakpoints.retain(|b| !b.once);
        remark(&table, &state);
    }
    // A patch may come while the task waits.
    drop(table);
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let stop =
        Stop { task, site, frame, bounds, addresses: addresses as usize, alive: alive as usize, why: String::new() };
    drop(quiet);
    hold(state, stop, why);
    restarting(frame, task, site)
}

/// In a replay, the probe of the program's thread a `seek` or a `scan` goes
/// to; while scanning, the probes where breakpoints would have stopped,
/// and those where the lines of one task start.
pub(super) static AIM: AtomicU64 = AtomicU64::new(u64::MAX);
pub(super) static SCANNING: AtomicBool = AtomicBool::new(false);
pub(super) static SCANNED: Mutex<(u64, std::collections::VecDeque<u64>, std::collections::VecDeque<u64>)> =
    Mutex::new((0, std::collections::VecDeque::new(), std::collections::VecDeque::new()));

/// What a scan keeps of each list, the last: what going back needs.
pub(super) const KEPT: usize = 10_000;

pub(super) fn kept_last(list: &mut std::collections::VecDeque<u64>, tick: u64) {
    if list.len() == KEPT {
        list.pop_front();
    }
    list.push_back(tick);
}

/// Forks the replay where its task stopped, on that task: the copy keeps
/// the program as it is here, without the debugger, and waits on a socket
/// of its own; each `resume <probe>` there forks it again into a replay
/// stopped here, which shows what the program writes from that probe on.
pub(super) fn checkpoint(table: &Table) -> Result<u32, String> {
    if crate::replay::threaded() {
        return Err("a copy of the replay would lose its threads of C".into());
    }
    // What the debugger's thread may hold is held across the fork, so that
    // the copy, without that thread, has it free: it asks for this one by
    // `on_task`, and may still be on its way to wait for it.
    let locks = (state(), CLIENT.lock(), WORKED.lock(), WORK.lock());
    let forked = crate::tasks::fork_copy();
    drop(locks);
    match forked {
        Err(error) => Err(format!("cannot fork the replay: {error}")),
        Ok(0) => {
            kept(table);
            crate::diagnostics::listen_again();
            Ok(0)
        }
        Ok(pid) => Ok(pid as u32),
    }
}

/// In a replay, a copy of the process where its task stopped, which the
/// debugger goes back to rather than to the start. This thread holds no
/// lock of the table meanwhile: the copy, without it, could never add a
/// patch's.
pub(super) fn take_checkpoint() -> String {
    let Some(stop) = state().stopped.clone().filter(|s| s.site < WAITING) else {
        return "{\"error\":\"the program runs: `pause` stops it, or a breakpoint\"}".to_string();
    };
    let tick = crate::replay::ticks();
    let taken = on_task(stop.task, || match table() {
        Some(table) => checkpoint(&table),
        None => Err("the program is not a debug build".to_string()),
    });
    match taken {
        Some(Ok(pid)) => format!("{{\"checkpoint\":{{\"pid\":{pid},\"tick\":{tick}}}}}"),
        Some(Err(message)) => format!("{{\"error\":{}}}", quoted(&message)),
        None => "{\"error\":\"the stopped task did not take the checkpoint\"}".into(),
    }
}

/// In a replay, how often, in milliseconds, a program that runs keeps a
/// checkpoint at a probe (`checkpoints <ms>`), or never; and when it last
/// did.
pub(super) static EVERY: AtomicU64 = AtomicU64::new(0);
pub(super) static LAST: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
/// The probe at which to see next if a checkpoint is due.
pub(super) static CHECKPOINT_LOOK: AtomicU64 = AtomicU64::new(0);
/// So many probes make a checkpoint due too, as a replay may run them
/// faster than the time does.
pub(super) const PROBES: u64 = 1 << 22;

/// How far into a value a dump keeps its parts, and how many of each.
pub(super) const DUMPED_DEPTH: usize = 4;
pub(super) const DUMPED_PARTS: usize = 100;
/// How long a dump takes to show the bindings, and how many that break it
/// it goes past.
pub(super) const DUMPING: std::time::Duration = std::time::Duration::from_secs(3);
pub(super) const BROKEN: usize = 16;

/// This process is the copy of the program that shows a dump's bindings.
pub(super) static RENDERING: AtomicBool = AtomicBool::new(false);

pub(crate) fn rendering() -> bool {
    RENDERING.load(Ordering::Relaxed)
}

/// The parts of a value as a dump keeps them, to open it afterwards.
pub(super) fn dumped_parts(text: &str, depth: usize) -> String {
    let part = |(name, part): &(String, crate::shown::Part)| {
        let parts = dumped_parts(part.text, depth - 1);
        format!("{{\"name\":{},\"value\":{},\"parts\":{parts}}}", quoted(name), quoted(&clipped(part.text)))
    };
    let parts = match depth {
        0 => Vec::new(),
        _ => crate::shown::parse(text).parts.iter().take(DUMPED_PARTS).map(part).collect(),
    };
    format!("[{}]", parts.join(","))
}

/// The frame of the code at `pc`, whose frame pointer is `fp`.
pub(super) fn frame_at(table: &Table, pc: usize, fp: usize) -> Option<Frame> {
    let code = crate::trace::describe(pc + 1)?;
    let (file, line) = code.at?;
    let function = table.functions.iter().position(|f| f.name == code.function)?;
    let (addressed, living) =
        table.offsets.get(function).map(|(a, l)| (a.load(Ordering::Relaxed), l.load(Ordering::Relaxed)))?;
    let (addresses, alive) = match addressed {
        UNKNOWN => (0, 0),
        _ => ((fp as isize + addressed) as usize, (fp as isize + living) as usize),
    };
    let place = format!("{file}:{line} in {}", code.function);
    Some(Frame { place, function: Some(function), pointer: fp, addresses, alive, pc })
}

/// The bindings of every frame of every task, for a dump: a line of JSON
/// each. Showing a value reads memory that may be broken, at a crash above
/// all, and takes locks others may hold: a copy of the program does it,
/// which may crash or hang without harm, and one that crashes goes on from
/// the next binding. `here` is where this thread's task stands.
pub(crate) fn values(here: Option<(Option<usize>, usize, (usize, usize))>) -> Vec<String> {
    rendered(here, false)
}

/// What the frame at the site `pc` holds, for a logpoint: as `values`, of
/// that frame alone, whose registers are all known.
pub(crate) fn frame_values(
    pc: usize,
    fp: usize,
    bounds: (usize, usize),
    registers: &[usize; 32],
    floats: &[usize; 32],
    condition: Option<String>,
) -> Vec<String> {
    *CONDITION.lock().unwrap_or_else(|e| e.into_inner()) = condition;
    site_registers(registers, floats, true);
    let found = rendered(Some((Some(pc), fp, bounds)), true);
    site_registers(registers, floats, false);
    found
}

/// While a logpoint shows what its site holds, the registers there are known.
pub(crate) fn site_registers(registers: &[usize; 32], floats: &[usize; 32], known: bool) {
    crate::diagnostics::fault_registers(registers, known);
    FLOATS.iter().zip(floats).for_each(|(slot, &value)| slot.store(value, Ordering::Relaxed));
    LOGGING.store(known, Ordering::Relaxed);
}

/// Whether an expression reads.
pub(crate) fn valid_expression(text: &str) -> Result<(), String> {
    expression::parse(text).map(drop)
}

/// What must hold for the frame a logpoint shows to be shown.
pub(super) static CONDITION: Mutex<Option<String>> = Mutex::new(None);

/// While a logpoint shows a frame, the low half of each register of numbers.
pub(super) static LOGGING: AtomicBool = AtomicBool::new(false);
pub(super) static FLOATS: [std::sync::atomic::AtomicUsize; 32] = [const { std::sync::atomic::AtomicUsize::new(0) }; 32];

/// Each place of the code where a logpoint can go: its address, its length,
/// its file and its line.
pub(crate) fn latent_sites() -> Vec<(usize, usize, String, u32, usize)> {
    let raw = DUMP_TABLE.load(Ordering::Relaxed);
    if raw.is_null() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(unsafe { str_bytes((*raw).text) }).into_owned();
    let (mut sites, mut function) = (Vec::new(), None);
    for line in text.lines() {
        match line.split('\t').collect::<Vec<_>>()[..] {
            ["F", index] => function = index.parse().ok().and_then(crate::trace::function_start),
            ["P", at, length, file, line] => {
                if let (Some(start), Ok(at), Ok(length), Ok(line)) =
                    (function, at.parse::<usize>(), length.parse(), line.parse())
                {
                    sites.push((start + at, length, file.to_string(), line, start));
                }
            }
            _ => {}
        }
    }
    sites
}

pub(super) fn rendered(here: Option<(Option<usize>, usize, (usize, usize))>, alone: bool) -> Vec<String> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let table = table();
    if table.is_none() && DUMP_TABLE.load(Ordering::Relaxed).is_null() {
        return Vec::new();
    }
    let _outside = crate::replay::Outside::enter();
    let deadline = std::time::Instant::now() + DUMPING;
    let (mut lines, mut skipped) = (Vec::new(), 0);
    for _ in 0..BROKEN {
        let mut ends = [0; 2];
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            break;
        }
        match unsafe { libc::fork() } {
            -1 => break,
            0 => {
                unsafe { libc::close(ends[0]) };
                render(table.as_deref(), here, lines.len() + skipped, ends[1], alone);
                unsafe { libc::_exit(0) };
            }
            pid => {
                unsafe { libc::close(ends[1]) };
                unsafe { libc::fcntl(ends[0], libc::F_SETFL, libc::O_NONBLOCK) };
                let mut pipe = unsafe { std::fs::File::from_raw_fd(ends[0]) };
                let (mut text, mut part, mut ended) = (Vec::new(), [0u8; 8192], false);
                while !ended && std::time::Instant::now() < deadline {
                    match pipe.read(&mut part) {
                        Ok(0) => ended = true,
                        Ok(read) => text.extend_from_slice(&part[..read]),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            let mut ready = libc::pollfd { fd: ends[0], events: libc::POLLIN, revents: 0 };
                            unsafe { libc::poll(&mut ready, 1, 50) };
                        }
                        Err(_) => ended = true,
                    }
                }
                let mut status = 0;
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, &mut status, 0);
                }
                // What came whole: a line cut short has no end.
                let end = text.iter().rposition(|&b| b == b'\n').map_or(0, |at| at + 1);
                let whole = text[..end].split(|&b| b == b'\n').filter(|line| !line.is_empty());
                lines.extend(whole.map(|line| String::from_utf8_lossy(line).into_owned()));
                // It ended by itself, or it stands at the binding that
                // broke it: on past that one.
                let done = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
                if done || !ended {
                    break;
                }
                skipped += 1;
            }
        }
    }
    lines
}

/// Where a task stands in a dump: its id, the code of a fault, its frame,
/// its stack, and for one switched out, its registers and where it was.
pub(super) type TaskStop = (u64, Option<usize>, usize, (usize, usize), Option<crate::unwinding::Switched>);

/// In the copy of the program, shows the bindings from the `from`th on,
/// a line each on `out`.
pub(super) fn render(
    table: Option<&Table>,
    here: Option<(Option<usize>, usize, (usize, usize))>,
    from: usize,
    out: i32,
    alone: bool,
) {
    RENDERING.store(true, Ordering::Relaxed);
    // A value that breaks this copy only skips that binding: at once, as
    // a core takes seconds, and without a word on the program's stderr.
    extern "C" fn broke(_: i32) {
        unsafe { libc::_exit(1) };
    }
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = broke as *const () as usize;
        action.sa_flags = libc::SA_ONSTACK;
        for signal in [libc::SIGSEGV, libc::SIGBUS, libc::SIGABRT, libc::SIGILL, libc::SIGFPE] {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
        libc::setrlimit(libc::RLIMIT_CORE, &libc::rlimit { rlim_cur: 0, rlim_max: 0 });
        libc::dup2(libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY), 2);
    }
    let _quiet = crate::heap::Quiet::enter();
    let mut stops: Vec<TaskStop> =
        here.into_iter().map(|(pc, fp, bounds)| (crate::tasks::current_id(), pc, fp, bounds, None)).collect();
    if !alone {
        stops.extend(crate::tasks::standing().into_iter().map(|(id, fp, bounds, saved)| (id, None, fp, bounds, saved)));
    }
    let mut index = 0;
    // A logpoint's condition reads all the frame has, so what it shows
    // waits for that.
    let condition = alone.then(|| CONDITION.lock().unwrap_or_else(|e| e.into_inner()).clone()).flatten();
    let mut waiting: Vec<(String, String, String)> = Vec::new();
    let mut show = |task: u64, frame: usize, place: &str, name: &str, ty: &str, glue: Show, address: usize| {
        index += 1;
        if index <= from {
            return;
        }
        let text = shown_here(glue, address);
        let line = format!(
            "{{\"task\":{task},\"frame\":{frame},\"at\":{},\"name\":{},\"type\":{},\"value\":{},\"parts\":{}}}\n",
            quoted(place),
            quoted(name),
            quoted(ty),
            quoted(&clipped(&text)),
            dumped_parts(&text, DUMPED_DEPTH),
        );
        match condition.is_some() {
            true => waiting.push((name.to_string(), text, line)),
            false => drop(unsafe { libc::write(out, line.as_ptr().cast(), line.len()) }),
        }
    };
    let release = table.is_none().then(dump_functions);
    for (task, pc, frame, bounds, switched) in stops {
        // Past the frames of the Rust library, which may have no pointers.
        let frame = if pc.is_none() { crate::trace::through(frame, bounds) } else { frame };
        if let Some(table) = table {
            let stop = Stop { task, site: WAITING, frame, bounds, addresses: 0, alive: 0, why: String::new() };
            let mut stack = frames(table, &stop);
            stack.splice(0..0, pc.and_then(|pc| frame_at(table, pc, frame)));
            for at in 0..stack.len() {
                for (binding, address, _) in visible(table, &stack, at) {
                    show(task, at, &stack[at].place, &binding.name, &binding.ty, binding.show, address);
                }
            }
        } else if let Some(functions) = &release {
            let top = pc.map(|pc| (frame, pc + 1));
            let (mut at, mut registers): (_, Registers) = (0, [None; 32]);
            // A fault says all of them, for the frame it stopped.
            if pc.is_some() {
                registers = crate::diagnostics::faulted_registers();
            }
            // One that switched out left them where it did, in the
            // runtime's frame that called the switch.
            if let Some((saved, address)) = switched {
                registers = crate::unwinding::restored(address, frame, &saved, bounds);
            }
            let (kept_at, kept) = PANIC_REGISTERS.get();
            for (fp, address) in top.into_iter().chain(crate::trace::callers(frame, bounds)) {
                // The runtime's frames saved what the program's code had.
                let Some((function, start)) = crate::trace::function_at(address) else {
                    registers = crate::unwinding::restored(address, fp, &registers, bounds);
                    continue;
                };
                if fp == kept_at {
                    for (&register, &value) in CALLEE_SAVED.iter().zip(&kept) {
                        registers[register] = Some(value);
                    }
                }
                let offset = (address - 1 - start) as u32;
                let found = functions.get(&function);
                // What a panic kept is in this frame, if it panicked.
                let numbers = PANIC_NUMBERS.get().addr();
                let numbers = (fp == kept_at && numbers != 0).then_some(numbers);
                let scopes = crate::trace::scopes_at(address);
                for (record, place) in scopes.iter().filter(|(_, place)| !place.contains(" in spawn")) {
                    let line = place.split(" in ").next().and_then(|at| at.rsplit(':').next());
                    let line: u32 = line.and_then(|line| line.parse().ok()).unwrap_or(u32::MAX);
                    let bound = found.iter().flat_map(|f| &f.bindings);
                    for binding in bound.filter(|b| b.scope == *record && b.from <= line) {
                        let mut word = 0usize;
                        let kept = numbers.zip(binding.keep).and_then(|(numbers, index)| {
                            let bits = unsafe { *(numbers as *const u64) };
                            (bits & (1 << index) != 0).then_some(numbers + (index + 1) * 8)
                        });
                        if let Some(address) = kept.or_else(|| binding.found(fp, offset, &registers, &mut word)) {
                            show(task, at, place, &binding.name, &binding.ty, binding.show, address);
                        }
                    }
                    at += 1;
                }
                if alone {
                    break;
                }
                let saved = found.map_or(&[][..], |f| &f.saved);
                registers = caller_registers(&registers, fp, saved);
            }
        }
    }
    let Some(condition) = &condition else { return };
    let read = |name: &str| {
        let found = waiting.iter().find(|(binding, ..)| binding == name);
        found.map(|(_, text, _)| text.clone()).ok_or(format!("no binding `{name}` holds a value here"))
    };
    if expression::parse(condition).is_ok_and(|expr| Evaluator::new(&read).holds(&expr)) {
        for (.., line) in &waiting {
            unsafe { libc::write(out, line.as_ptr().cast(), line.len()) };
        }
    }
}

/// What a build without probes says of a binding, for its dumps.
pub(super) struct DumpBinding {
    pub(super) name: String,
    pub(super) ty: String,
    pub(super) show: Show,
    /// Where it is from the frame pointer, when it has a slot there.
    pub(super) slot: Option<isize>,
    /// Behind what the ranges find, a pointer to it.
    pub(super) pointer: bool,
    /// Over which code it is where.
    pub(super) ranges: Vec<(u32, u32, Place)>,
    /// Its place among what an explicit panic keeps.
    pub(super) keep: Option<usize>,
    /// The inlined call it is of, or `u32::MAX`.
    pub(super) scope: u32,
    /// The first line that gives a slot its value: before it, what is there
    /// is not the binding's.
    pub(super) from: u32,
}

/// Where a value is: at an offset from the canonical frame address, or in
/// a register, by its number.
#[derive(Clone, Copy)]
pub(super) enum Place {
    Frame(isize),
    Register(usize),
    /// A register of numbers, which only a logpoint saves.
    Float(usize),
}

/// What a dump reads of a function of a build without probes.
#[derive(Default)]
pub(super) struct DumpFunction {
    pub(super) bindings: Vec<DumpBinding>,
    /// Where it saves its caller's registers, from the canonical frame
    /// address, by number.
    pub(super) saved: Vec<(usize, isize)>,
}

/// The registers of a frame, those known.
use crate::unwinding::Registers;

/// The callee-saved registers, as the ABI says.
#[cfg(target_arch = "x86_64")]
pub(super) const CALLEE_SAVED: &[usize] = &[3, 12, 13, 14, 15];
#[cfg(target_arch = "aarch64")]
pub(super) const CALLEE_SAVED: &[usize] = &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28];

thread_local! {
    /// The frame pointer and callee-saved registers before a panic.
    pub(super) static PANIC_REGISTERS: std::cell::Cell<(usize, [usize; 10])> = const { std::cell::Cell::new((0, [0; 10])) };
}

thread_local! {
    /// What an explicit panic kept: the bits of those it holds, then each.
    pub(super) static PANIC_NUMBERS: std::cell::Cell<*const u64> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// Called before a panic the program wrote, with what it keeps.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_keep_numbers(kept: *const u64) {
    PANIC_NUMBERS.set(kept);
}

/// Called by what keeps the registers: the frame pointer of the code that
/// panics, then the callee-saved registers.
pub(super) extern "C" fn registers_kept(registers: *const usize) {
    let mut kept = [0; 10];
    let fp = unsafe { *registers };
    for (index, slot) in kept.iter_mut().enumerate().take(CALLEE_SAVED.len()) {
        *slot = unsafe { *registers.add(index + 1) };
    }
    PANIC_REGISTERS.set((fp, kept));
}

/// Keeps the registers of the code that is about to panic, which the
/// runtime's own frames would overwrite on the way to a dump.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn tondo_rt_keep_registers() {
    std::arch::naked_asm!(
        "sub rsp, 56",
        "mov [rsp], rbp",
        "mov [rsp + 8], rbx",
        "mov [rsp + 16], r12",
        "mov [rsp + 24], r13",
        "mov [rsp + 32], r14",
        "mov [rsp + 40], r15",
        "mov rdi, rsp",
        "call {kept}",
        "add rsp, 56",
        "ret",
        kept = sym registers_kept,
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn tondo_rt_keep_registers() {
    std::arch::naked_asm!(
        "sub sp, sp, #112",
        "stp x29, x19, [sp]",
        "stp x20, x21, [sp, #16]",
        "stp x22, x23, [sp, #32]",
        "stp x24, x25, [sp, #48]",
        "stp x26, x27, [sp, #64]",
        "stp x28, x30, [sp, #80]",
        "mov x0, sp",
        "bl {kept}",
        "ldr x30, [sp, #88]",
        "add sp, sp, #112",
        "ret",
        kept = sym registers_kept,
    )
}

impl DumpBinding {
    /// Where the binding is at a place of the code of the frame at `fp`:
    /// in memory, or a copy of a register in `word`.
    pub(super) fn found(&self, fp: usize, offset: u32, registers: &Registers, word: &mut usize) -> Option<usize> {
        if let Some(slot) = self.slot {
            return Some((fp as isize + slot) as usize);
        }
        let &(.., place) = self.ranges.iter().find(|&&(start, end, _)| start <= offset && offset < end)?;
        let value = match place {
            Place::Frame(at) => {
                let address = (fp as isize + 16 + at) as usize;
                if !self.pointer {
                    return Some(address);
                }
                peek(address, 8).try_into().ok().map(usize::from_ne_bytes)?
            }
            Place::Register(number) => (*registers.get(number)?)?,
            Place::Float(number) => {
                let known = LOGGING.load(Ordering::Relaxed);
                known.then(|| FLOATS.get(number).map(|float| float.load(Ordering::Relaxed)))??
            }
        };
        if self.pointer {
            return Some(value);
        }
        *word = value;
        Some(word as *mut usize as usize)
    }
}

/// The registers of the caller of the frame at `fp`, of a function that
/// saves these.
pub(super) fn caller_registers(registers: &Registers, fp: usize, saved: &[(usize, isize)]) -> Registers {
    let mut caller: Registers = [None; 32];
    for &register in CALLEE_SAVED {
        caller[register] = registers[register];
    }
    for &(register, at) in saved {
        let word = peek((fp as isize + 16 + at) as usize, 8).try_into().ok().map(usize::from_ne_bytes);
        if let Some(slot) = caller.get_mut(register) {
            *slot = word;
        }
    }
    caller
}

/// The table of what the dumps of a build without probes show.
pub(super) static DUMP_TABLE: AtomicPtr<Raw> = AtomicPtr::new(std::ptr::null_mut());

/// Called by a program without probes before anything else.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_dump_table(raw: *mut Raw) {
    DUMP_TABLE.store(raw, Ordering::Relaxed);
}

/// The bindings of each function, by its number, as `tondo_rt_dump_table`
/// has them, which only a dump reads.
pub(super) fn dump_functions() -> std::collections::HashMap<usize, DumpFunction> {
    let mut functions: std::collections::HashMap<usize, DumpFunction> = Default::default();
    let raw = DUMP_TABLE.load(Ordering::Relaxed);
    if raw.is_null() {
        return functions;
    }
    let raw = unsafe { &*raw };
    let text = String::from_utf8_lossy(unsafe { str_bytes(raw.text) }).into_owned();
    let glues = unsafe { std::slice::from_raw_parts(raw.glues, raw.count as usize) };
    let (mut current, mut next) = (0, 0);
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let function = functions.entry(current).or_default();
        match fields[..] {
            ["F", index] => current = index.parse().unwrap_or(0),
            ["C", register, at] => {
                if let (Ok(register), Ok(at)) = (register.parse(), at.parse()) {
                    function.saved.push((register, at));
                }
            }
            ["B", name, ty, kind, value, keep, scope, from, _] => {
                let (name, ty) = (name.to_string(), ty.to_string());
                let show = glues[next];
                next += 1;
                let value: isize = value.parse().unwrap_or(0);
                let (slot, pointer) = if kind == "s" { (Some(value), false) } else { (None, value == 1) };
                let keep = keep.parse().ok();
                let scope = scope.parse().unwrap_or(u32::MAX);
                let from = from.parse().unwrap_or(0);
                let ranges = Vec::new();
                function.bindings.push(DumpBinding { name, ty, show, slot, pointer, ranges, keep, scope, from });
            }
            ["R", start, end, kind, at] => {
                let place = match kind {
                    "f" => at.parse().ok().map(Place::Frame),
                    "x" => at.parse().ok().map(Place::Float),
                    _ => at.parse().ok().map(Place::Register),
                };
                if let (Some(binding), Ok(start), Ok(end), Some(place)) =
                    (function.bindings.last_mut(), start.parse(), end.parse(), place)
                {
                    binding.ranges.push((start, end, place));
                }
            }
            _ => {}
        }
    }
    functions
}

/// A checkpoint at a probe of a replay that runs, when one is due: the
/// debugger's state and connection held across the fork, so that the copy
/// has none another thread held; the debugger hears where it is. A replay
/// forked from it later stops at that probe.
pub(super) fn periodic(table: &Table, mut here: Stop, every: u64) -> Option<Stop> {
    if crate::replay::threaded() {
        return None;
    }
    let (now, tick) = (std::time::Instant::now(), crate::replay::ticks());
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let due = |(at, probe): (std::time::Instant, u64)| {
            now.duration_since(at).as_millis() >= every as u128 || tick - probe >= PROBES
        };
        if last.is_some_and(|last| !due(last)) {
            return None;
        }
        *last = Some((now, tick));
    }
    let busy = idle_debugger()?;
    let state = state();
    let client = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    // A debugger that left would never hear of it.
    if client.is_none() {
        return None;
    }
    match crate::tasks::fork_copy() {
        Ok(0) => {
            drop((busy, client, state));
            kept(table);
            here.bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
            Some(here)
        }
        Err(_) => None,
        Ok(pid) => {
            drop((busy, client, state));
            tell(&format!("{{\"checkpointed\":{{\"pid\":{pid},\"tick\":{}}}}}", crate::replay::ticks()));
            None
        }
    }
}

/// While the debugger's thread runs a command it may hold the table, which
/// a copy could never have back: a copy waits for one that does not run,
/// and a command for a copy that is being taken. Not a lock: the copy that
/// a command itself asks for has to be able to clear it.
pub(super) static COMMANDING: AtomicBool = AtomicBool::new(false);
pub(super) static COPYING: AtomicBool = AtomicBool::new(false);

pub(super) struct Commanding;

impl Commanding {
    pub(super) fn enter() -> Commanding {
        COMMANDING.store(true, Ordering::SeqCst);
        while COPYING.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        Commanding
    }
}

impl Drop for Commanding {
    fn drop(&mut self) {
        COMMANDING.store(false, Ordering::SeqCst);
    }
}

pub(super) struct Copying;

impl Drop for Copying {
    fn drop(&mut self) {
        COPYING.store(false, Ordering::SeqCst);
    }
}

pub(super) fn idle_debugger() -> Option<Copying> {
    COPYING.store(true, Ordering::SeqCst);
    let copying = Copying;
    (!COMMANDING.load(Ordering::SeqCst)).then_some(copying)
}

/// Stands a copy at its checkpoint, without the table its caller read: a
/// patch may come, which needs it. Gives it back.
pub(super) fn at_checkpoint(
    table: std::sync::RwLockReadGuard<'static, Table>,
    stop: Stop,
) -> Option<std::sync::RwLockReadGuard<'static, Table>> {
    drop(table);
    hold(state(), stop, Some(CHECKPOINT.into()));
    self::table()
}

/// Why a replay forked from a checkpoint is stopped.
pub(super) const CHECKPOINT: &str = "checkpoint";

/// A copy of a run being recorded, at a probe: as a checkpoint, held until
/// a replay goes on from here, of what was recorded after it.
pub(super) fn copied(site: usize, frame: usize, addresses: usize, alive: usize) -> Option<Stop> {
    let table = table()?;
    let _outside = crate::replay::Outside::enter();
    let task = crate::tasks::current_id();
    let busy = idle_debugger()?;
    let state = state();
    let client = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let mut parent = Some((busy, client, state));
    let mut here = None;
    crate::replay::copy(|segment| {
        drop(parent.take());
        let debugger = kept(&table);
        crate::replay::replay_from(segment, debugger);
        let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
        here = Some(Stop { task, site, frame, bounds, addresses, alive, why: String::new() });
    });
    here
}

/// A checkpoint, until a replay forked from it goes on from here, which
/// returns. Only this thread is left: the debugger's connection and its
/// settings were the parent's. Gives the process of the debugger that
/// asked.
pub(super) fn kept(table: &Table) -> i32 {
    use std::io::Write;
    COMMANDING.store(false, Ordering::SeqCst);
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    tools::listen(tools::DEBUGGER, false);
    {
        let mut state = state();
        state.breakpoints.clear();
        state.watches.clear();
        state.next_breakpoint = 1;
        remark(table, &state);
    }
    WATCHING.store(false, Ordering::Relaxed);
    EVERY.store(0, Ordering::Relaxed);
    RECORDING.store(0, Ordering::Relaxed);
    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).clear();
    CATCH.store(true, Ordering::Relaxed);
    CATCH_ERRORS.store(false, Ordering::Relaxed);
    STEPS_STD.store(false, Ordering::Relaxed);
    // The replays it forks end without it waiting for them. What it has
    // open but its standard streams is its parent's: the sockets of the
    // tools, which would seem to listen once the parent ended.
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };
    let open = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) }.clamp(1024, 1 << 16) as i32;
    for fd in 3..open {
        unsafe { libc::close(fd) };
    }
    // Nor does it keep its parent's output open: each replay it forks
    // writes where the one that asks for it says.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        libc::dup2(null, 0);
        libc::dup2(null, 1);
        libc::dup2(null, 2);
        libc::close(null);
    }
    let path = crate::diagnostics::sockets().join(format!("{}.checkpoint", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = std::os::unix::net::UnixListener::bind(&path) else { unsafe { libc::_exit(0) } };
    // One the debugger never heard of, or forgot, ends after an hour
    // unused, or what `--hold` says; any, once the debugger is gone.
    let mut idle = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&listener), events: libc::POLLIN, revents: 0 };
    let mut unused = std::time::Duration::ZERO;
    loop {
        match unsafe { libc::poll(&mut idle, 1, 10_000) } {
            0 => {
                unused += std::time::Duration::from_secs(10);
                if unused >= crate::replay::hold_for() || crate::replay::orphaned() {
                    let _ = std::fs::remove_file(&path);
                    unsafe { libc::_exit(0) };
                }
                continue;
            }
            // A signal came.
            ..0 => continue,
            _ => unused = std::time::Duration::ZERO,
        }
        let Ok((mut stream, _)) = listener.accept() else { continue };
        let (line, given) = received(&stream);
        let asked = line.trim().strip_prefix("resume ").and_then(|asked| asked.split_once(' '));
        let Some((shown, debugger)) = asked.and_then(|(s, d)| Some((s.parse::<u64>().ok()?, d.parse::<i32>().ok()?)))
        else {
            let _ = std::fs::remove_file(&path);
            unsafe { libc::_exit(0) };
        };
        match unsafe { libc::fork() } {
            0 => {
                drop(listener);
                unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) };
                // Its output, where the one that asked for it said.
                if let [out, error] = given[..] {
                    unsafe {
                        libc::dup2(out, 1);
                        libc::dup2(error, 2);
                    }
                }
                given.iter().for_each(|&fd| unsafe {
                    libc::close(fd);
                });
                crate::replay::shown_from(shown);
                return debugger;
            }
            pid => {
                given.iter().for_each(|&fd| unsafe {
                    libc::close(fd);
                });
                let _ = writeln!(stream, "{pid}");
            }
        }
    }
}

/// A line a checkpoint was sent, with the descriptors that came with it.
pub(super) fn received(stream: &std::os::unix::net::UnixStream) -> (String, Vec<i32>) {
    let fd = std::os::fd::AsRawFd::as_raw_fd(stream);
    let mut data = [0u8; 256];
    let mut control = [0u8; 64];
    let mut part = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let read = unsafe { libc::recvmsg(fd, &mut message, 0) };
    let mut given = Vec::new();
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while let Some(found) = header.as_ref() {
            if found.cmsg_level == libc::SOL_SOCKET && found.cmsg_type == libc::SCM_RIGHTS {
                // musl's length is a `u32`.
                #[allow(clippy::unnecessary_cast)]
                let count = (found.cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<i32>();
                let fds = libc::CMSG_DATA(header).cast::<i32>();
                given.extend((0..count).map(|i| fds.add(i).read_unaligned()));
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    (String::from_utf8_lossy(&data[..read.max(0) as usize]).into_owned(), given)
}

/// What a scan follows as it goes (`track`, `count`): expressions, each in
/// a frame, whose values it keeps when they change, each change with the
/// probe and the line before it, which gave it, whatever task ran it; and
/// for one line, each time a task starts it, with an expression's value
/// then, the last ones.
#[derive(Default)]
pub(super) struct Tracked {
    pub(super) followed: Vec<Followed>,
    pub(super) before: Option<(u64, usize)>,
    pub(super) line: Option<Counted>,
    /// Each task's last line of the program's code, by probe and site.
    pub(super) lines: std::collections::BTreeMap<u64, (u64, usize)>,
    /// Where each value a channel holds was sent from, the oldest first:
    /// a line and its task; `None` for one sent before the scan began.
    pub(super) messages: std::collections::BTreeMap<usize, std::collections::VecDeque<Option<Origin>>>,
    /// The line each task ended on, by its join.
    pub(super) ends: std::collections::BTreeMap<usize, Origin>,
    /// What a line received, by its probe and site: where it was sent from,
    /// or where the task it waited for ended; the last ones.
    pub(super) received: std::collections::VecDeque<((u64, usize), Origin)>,
    /// Where each line of the program's code that runs goes: its task, its
    /// probe and its site, which another run of the program compares.
    pub(super) trail: Option<std::io::BufWriter<std::fs::File>>,
    /// What went wrong writing it, which the scan says when it ends.
    pub(super) trail_failed: Option<String>,
}

/// A line of a task: its probe, its site and the task.
pub(super) type Origin = (u64, usize, u64);

/// What a scan keeps of a value that changes hands: `from` is the task
/// whose line it comes from, `to` the one whose line takes it; `None` when
/// what was sent before the scan began.
pub(super) fn handed_over(from: impl FnOnce(&mut Tracked) -> Option<Origin>, to: u64) {
    // The scan's own work, which the recording does not have.
    let _outside = crate::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let (Some(origin), Some(&line)) = (from(tracked), tracked.lines.get(&to)) else { return };
    if tracked.received.len() == LISTED {
        tracked.received.pop_front();
    }
    tracked.received.push_back((line, origin));
}

pub(super) fn origin_of(tracked: &Tracked, task: u64) -> Option<Origin> {
    tracked.lines.get(&task).map(|&(tick, site)| (tick, site, task))
}

/// The task `from` sent a value into the channel, which held `held`.
pub(crate) fn sent(channel: usize, held: usize, from: u64) {
    let _outside = crate::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let origin = origin_of(tracked, from);
    tracked.messages.entry(channel).or_insert_with(|| vec![None; held].into()).push_back(origin);
}

/// A task ended, and its stack goes: a scan follows its frames no longer.
pub(crate) fn task_ended(task: u64) {
    let _outside = crate::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    for followed in tracked.iter_mut().flat_map(|t| &mut t.followed).filter(|f| f.anchor.task == task) {
        followed.anchor.live = None;
    }
}

/// The task `by` took the oldest value of the channel, which held `held`.
pub(crate) fn taken(channel: usize, held: usize, by: u64) {
    let take = |t: &mut Tracked| t.messages.entry(channel).or_insert_with(|| vec![None; held].into()).pop_front()?;
    handed_over(take, by);
}

/// The task `from` gave a value to `to`, which waited for it.
pub(crate) fn handed(from: u64, to: u64) {
    handed_over(|t| origin_of(t, from), to);
}

/// The current task ends, its result in `join`.
pub(crate) fn ended(join: usize) {
    let _outside = crate::replay::Outside::enter();
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tracked) = tracked.as_mut()
        && let Some(origin) = origin_of(tracked, crate::tasks::current_id())
    {
        tracked.ends.insert(join, origin);
    }
}

/// The current task takes the result in `join`.
pub(crate) fn joined(join: usize) {
    handed_over(|t| t.ends.get(&join).copied(), crate::tasks::current_id());
}

/// A frame as another replay of the same run finds it, where addresses
/// may differ: its task, how far its frame is from the top of the task's
/// stack, and its function; and where it is while it lives.
pub(super) struct Anchor {
    pub(super) task: u64,
    pub(super) depth: usize,
    pub(super) function: usize,
    pub(super) live: Option<usize>,
}

impl Anchor {
    pub(super) fn parse(text: &str) -> Option<Anchor> {
        let mut parts = text.split(':').map(str::parse::<u64>);
        let (Some(Ok(task)), Some(Ok(depth)), Some(Ok(function)), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        Some(Anchor { task, depth: depth as usize, function: function as usize, live: None })
    }
}

/// `anchor [<expression>]`: the frame `locals` looks at, as `track` takes
/// it; or the one further down the stack that has the binding the
/// expression reads, as a closure's maker.
pub(super) fn anchor(table: &Table, text: &str) -> Result<String, String> {
    let (frames, mut index) = selected(table)?;
    if !text.is_empty() {
        let expr = expression::parse(text).map_err(|m| format!("the expression: {m}"))?;
        let mut read = Vec::new();
        expression::names(&expr, &mut read);
        let name = |frame: &Frame| frame.function.map(|f| table.functions[f].name.as_str());
        let has = |frame: &Frame| read.first().is_some_and(|r| bound(table, frame).iter().any(|(b, _)| &b.name == r));
        // As `visible` finds it.
        while !has(&frames[index]) {
            let Some((outer, prefix)) = name(&frames[index]).and_then(written_in) else { break };
            let below = (index + 1..frames.len()).find(|&below| {
                name(&frames[below]).is_some_and(|n| if prefix { n.starts_with(outer) } else { n == outer })
            });
            let Some(below) = below else { break };
            index = below;
        }
    }
    let state = state();
    let task = state.selected.map_or(state.stopped.as_ref().map_or(0, |s| s.task), |(task, _)| task);
    let high = stop_of(&state, task).map_or(0, |held| held.bounds.1);
    let frame = &frames[index];
    match (frame.function, high.checked_sub(frame.pointer)) {
        (Some(function), Some(depth)) if high != 0 => Ok(format!("{{\"anchor\":\"{task}:{depth}:{function}\"}}")),
        _ => Err("this frame is not one of the program's that a replay finds again".into()),
    }
}

/// An expression a scan follows in a frame: its text, its last value, and
/// its last changes, each with the probe and the site of the line that
/// gave it.
pub(super) struct Followed {
    pub(super) anchor: Anchor,
    pub(super) expr: Expr,
    pub(super) text: String,
    pub(super) last: Option<String>,
    pub(super) changes: std::collections::VecDeque<(Option<(u64, usize)>, String)>,
    /// The first change to each value, as the first time a condition held,
    /// which the last changes may no longer have.
    pub(super) firsts: Vec<(Option<(u64, usize)>, String)>,
}

/// A line a scan counts: its sites, the expression it shows, and its last
/// runs, each with the probe, the task and the value.
pub(super) struct Counted {
    pub(super) sites: Vec<usize>,
    pub(super) expr: Option<Expr>,
    pub(super) runs: std::collections::VecDeque<(u64, u64, Option<String>)>,
    pub(super) times: u64,
}

pub(super) static TRACKED: Mutex<Option<Tracked>> = Mutex::new(None);

/// At a probe of a scan, what it follows.
pub(super) fn tracking(table: &Table, here: Stop, tick: u64) {
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tracked) = tracked.as_mut() else { return };
    let (task, site) = (here.task, here.site);
    // The innermost frame alone, which is what a line's expression reads:
    // not the callers, whose names are looked up.
    let function = table.sites.get(site).map(|s| s.function);
    if let Some(line) = tracked.line.as_mut()
        && line.sites.contains(&site)
    {
        let innermost = [Frame {
            place: String::new(),
            function,
            pointer: here.frame,
            addresses: here.addresses,
            alive: here.alive,
            pc: 0,
        }];
        let value =
            line.expr.as_ref().and_then(|e| evaluating(table, &innermost, 0, |ev| ev.evaluate(e)).ok().map(|v| v.text));
        line.times += 1;
        if line.runs.len() == LISTED {
            line.runs.pop_front();
        }
        line.runs.push_back((tick, task, value));
    }
    // A followed frame lives from a probe of its own until its task runs
    // in a caller, or another call takes its place.
    let mut depth = None;
    for anchor in tracked.followed.iter_mut().map(|f| &mut f.anchor).filter(|a| a.task == task) {
        let depth = *depth
            .get_or_insert_with(|| crate::tasks::stack_bounds().map_or(0, |(_, high)| high).wrapping_sub(here.frame));
        if depth == anchor.depth && function == Some(anchor.function) {
            anchor.live = Some(here.frame);
        } else if depth <= anchor.depth {
            anchor.live = None;
        }
    }
    // A value changes between two lines of the program's code, of any task:
    // the first of them gave it, or a call it made.
    if table.sites.get(site).is_none_or(Site::generated) {
        return;
    }
    if let Some(trail) = tracked.trail.as_mut() {
        let _outside = crate::replay::Outside::enter();
        let line = [task, tick, site as u64].map(u64::to_le_bytes).concat();
        if let Err(wrong) = trail.write_all(&line) {
            tracked.trail_failed = Some(wrong.to_string());
        }
    }
    // A probe where a line goes on is of that line: what it did by then and
    // does next is one change of the line, as the next line's start tells.
    // Not after another task ran, as after a wait: the changes that task
    // made since its own last probe are its line's.
    if table.sites.get(site).is_some_and(|site| site.goes_on) && tracked.lines.get(&task).copied() == tracked.before {
        return;
    }
    let before = tracked.before.replace((tick, site));
    tracked.lines.insert(task, (tick, site));
    for followed in tracked.followed.iter_mut() {
        let anchor = &followed.anchor;
        let Some(pointer) = anchor.live else { continue };
        let Some((addressed, living)) = table.offsets.get(anchor.function) else { continue };
        if addressed.load(Ordering::Relaxed) == UNKNOWN {
            continue;
        }
        let at = |offset: &AtomicIsize| (pointer as isize + offset.load(Ordering::Relaxed)) as usize;
        let frame = [Frame {
            place: String::new(),
            function: Some(anchor.function),
            pointer,
            addresses: at(addressed),
            alive: at(living),
            pc: 0,
        }];
        let now = evaluating(table, &frame, 0, |ev| ev.evaluate(&followed.expr)).ok().map(|v| v.text);
        if let Some(value) = now.as_ref().filter(|now| Some(*now) != followed.last.as_ref()) {
            if followed.changes.len() == LISTED {
                followed.changes.pop_front();
            }
            followed.changes.push_back((before, value.clone()));
            if followed.firsts.len() < LISTED && !followed.firsts.iter().any(|(_, v)| v == value) {
                followed.firsts.push((before, value.clone()));
            }
        }
        followed.last = now;
    }
}

/// `track <frame> <expression>`, `count <file>:<line> [<expression>]` and
/// `tracked`: what the next scan follows, and what it found.
pub(super) fn track(table: &Table, word: &str, rest: &str) -> Result<String, String> {
    let mut tracked = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    match word {
        // Where the values tasks receive come from, as the next scan goes.
        "track" if rest.trim().is_empty() => {
            tools::listen(tools::FOLLOWING, true);
            tracked.get_or_insert_with(Tracked::default);
            Ok("{\"tracking\":true}".into())
        }
        "track" => {
            let (anchor, text) =
                rest.split_once(' ').ok_or("track takes a frame, as `anchor` gives it, and an expression")?;
            let anchor = Anchor::parse(anchor).ok_or("track takes a frame first, as `anchor` gives it")?;
            let expr = expression::parse(text.trim()).map_err(|m| format!("the expression: {m}"))?;
            tools::listen(tools::FOLLOWING, true);
            tracked.get_or_insert_with(Tracked::default).followed.push(Followed {
                anchor,
                expr,
                text: text.trim().to_string(),
                last: None,
                changes: Default::default(),
                firsts: Vec::new(),
            });
            Ok("{\"tracking\":true}".into())
        }
        "trail" => {
            let _outside = crate::replay::Outside::enter();
            let file = std::fs::File::create(rest).map_err(|e| format!("cannot write {rest}: {e}"))?;
            tracked.get_or_insert_with(Tracked::default).trail = Some(std::io::BufWriter::new(file));
            Ok("{\"trailing\":true}".into())
        }
        "count" => {
            let (at, text) = rest.split_once(' ').map_or((rest, None), |(at, text)| (at, Some(text.trim())));
            let sites = line_sites(table, at)?;
            let expr = text.map(expression::parse).transpose().map_err(|m| format!("the expression: {m}"))?;
            tracked.get_or_insert_with(Tracked::default).line =
                Some(Counted { sites, expr, runs: Default::default(), times: 0 });
            Ok("{\"counting\":true}".into())
        }
        _ => {
            tools::listen(tools::FOLLOWING, false);
            let mut found = tracked.take().unwrap_or_default();
            if let Some(mut trail) = found.trail.take() {
                let _outside = crate::replay::Outside::enter();
                std::io::Write::flush(&mut trail).map_err(|e| format!("cannot write the lines of the run: {e}"))?;
                if let Some(wrong) = found.trail_failed.take() {
                    return Err(format!("cannot write the lines of the run: {wrong}"));
                }
            }
            let received: Vec<String> = found
                .received
                .iter()
                .map(|&((line, here), (tick, site, task))| {
                    let (here, at) = (quoted(&place_in(table, here)), quoted(&place_in(table, site)));
                    format!(
                        "{{\"tick\":{line},\"at\":{here},\"from\":{{\"tick\":{tick},\"at\":{at},\"task\":{task}}}}}"
                    )
                })
                .collect();
            let followed: Vec<String> = found
                .followed
                .iter()
                .map(|Followed { text, changes, firsts, .. }| {
                    let shown = |(before, value): &(Option<(u64, usize)>, String)| {
                        let (at, tick) = match before {
                            Some((tick, site)) => (quoted(&place_in(table, *site)), tick.to_string()),
                            None => ("null".into(), "null".into()),
                        };
                        format!("{{\"at\":{at},\"tick\":{tick},\"value\":{}}}", quoted(&clipped(value)))
                    };
                    let changes: Vec<String> = changes.iter().map(shown).collect();
                    let firsts: Vec<String> = firsts.iter().map(shown).collect();
                    format!(
                        "{{\"expression\":{},\"changes\":[{}],\"firsts\":[{}]}}",
                        quoted(text),
                        changes.join(","),
                        firsts.join(",")
                    )
                })
                .collect();
            let line = found.line.map_or(String::new(), |Counted { sites, runs, times, .. }| {
                let runs: Vec<String> = runs
                    .iter()
                    .map(|(tick, task, value)| {
                        let value =
                            value.as_ref().map_or(String::new(), |v| format!(",\"value\":{}", quoted(&clipped(v))));
                        format!("{{\"task\":{task},\"tick\":{tick}{value}}}")
                    })
                    .collect();
                format!(",\"ran\":[{}],\"times\":{times},\"at\":{}", runs.join(","), quoted(&place_in(table, sites[0])))
            });
            Ok(format!("{{\"tracked\":[{}]{line},\"received\":[{}]}}", followed.join(","), received.join(",")))
        }
    }
}

/// What a scan keeps at a probe: the tick, when the scanned task's line
/// starts there or a breakpoint that stops would stop there.
pub(super) fn scanned(
    table: &Table,
    state: &mut State,
    task: u64,
    site: usize,
    frame: usize,
    (addresses, alive): (usize, usize),
    tick: u64,
) {
    let mut scan = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
    if scan.0 == task && table.sites.get(site).is_some_and(|s| !s.generated()) {
        kept_last(&mut scan.2, tick);
    }
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let here = Stop { task, site, frame, bounds, addresses, alive, why: String::new() };
    let stack = std::cell::OnceCell::new();
    let stack = || stack.get_or_init(|| frames(table, &here));
    for breakpoint in state.breakpoints.iter_mut().filter(|b| b.log.is_none() && !b.capture && b.event.is_none()) {
        let here = breakpoint.sites.contains(&site)
            && breakpoint.task.is_none_or(|t| t == task)
            && in_request(&breakpoint.request, task)
            && breakpoint.condition.as_ref().is_none_or(|c| evaluating(table, stack(), 0, |e| e.holds(c)));
        if here {
            breakpoint.count += 1;
            if breakpoint.hits.as_ref().is_none_or(|hits| hits.reached(breakpoint.count)) {
                kept_last(&mut scan.1, tick);
            }
        }
    }
}

/// At a probe of a task, its watches: one whose frame returned ends, and
/// the first whose value changed gives why the task stops: its expression,
/// its values, and the line that changed it.
pub(super) fn watched(table: &Table, state: &mut State, task: u64, frame: usize, site: usize) -> Option<String> {
    let mut changed = None;
    let mut ended = Vec::new();
    for watch in state.watches.iter_mut() {
        if watch.shared.is_none() {
            if watch.task != task {
                continue;
            }
            // The frame returned when the task runs in an outer one, or
            // another call took its place.
            let watched = &watch.frames[0];
            let function = table.sites.get(site).map(|s| s.function);
            if frame > watched.pointer || (frame == watched.pointer && function != watched.function) {
                tell(&format!("{{\"unwatched\":{},\"why\":\"its frame returned\"}}", watch.id));
                ended.push(watch.id);
                continue;
            }
        }
        // A cell's change stops the task that made it, at its next probe,
        // not another that runs meanwhile.
        if let Some((cell, ..)) = watch.shared
            && ![0, task].contains(&crate::sync::last_writer(cell))
        {
            continue;
        }
        if let Some((now, holds)) = watch.now(table).filter(|(now, _)| *now != watch.value) {
            let (before, after) = (clipped(&watch.value), clipped(&now));
            watch.value = now;
            watch.count += u64::from(holds);
            if holds && watch.hits.as_ref().is_none_or(|hits| hits.reached(watch.count)) {
                // A shared value changes in some task: the line is that of
                // the frame's own changes only.
                let at = match watch.shared {
                    None => format!(" at {}", place_in(table, changer(table, &watch.last, frame, site))),
                    Some(_) => String::new(),
                };
                changed
                    .get_or_insert(format!("watch {}: {} changed from {before} to {after}{at}", watch.id, watch.text));
            }
        }
        // The frames below the one the task runs in now returned.
        watch.last.retain(|&(outer, _)| outer > frame);
        watch.last.push((frame, site));
    }
    unwatch(state, |watch| ended.contains(&watch.id));
    changed
}

/// The site of the line that changed a value since the last probes of each
/// frame, `last`, with the task now at `site` in `frame`: that of the last
/// probe, but of a call that returned and could not change it, as one of
/// the standard library's or one with no loan, the line that called it.
fn changer(table: &Table, last: &[(usize, usize)], frame: usize, site: usize) -> usize {
    let function = |site: usize| table.sites.get(site).map(|s| s.function);
    let could = |&&(at, probe): &&(usize, usize)| {
        let alive = at > frame || (at == frame && function(probe) == function(site));
        alive || table.sites.get(probe).is_none_or(|s| !s.generated() && table.functions[s.function].lends)
    };
    last.iter().rev().find(could).or(last.last()).map_or(usize::MAX, |&(_, probe)| probe)
}

/// Holds the task until the program goes on: as the one that stopped,
/// `why`, or as one of those the stop holds.
pub(super) fn hold(mut state: std::sync::MutexGuard<'static, State>, mut stop: Stop, why: Option<String>) {
    // What the debugger has the task do meanwhile is not the program's.
    let _outside = crate::replay::Outside::enter();
    let task = stop.task;
    let frozen = |state: &State| state.frozen.contains(&task);
    // Where its code is: the call of the probe that stopped it.
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let code = crate::trace::callers(crate::trace::frame_pointer(), bounds).find(|&(fp, _)| fp == stop.frame);
    state.code.retain(|&(held, _)| held != task);
    state.code.extend(code.map(|(_, address)| (task, address)));
    if let Some(why) = why {
        stop.why = why;
        tell(&stopped_event(&stop));
        state.stopped = Some(stop.clone());
        state.mode = Mode::Run;
        HOLD.store(true, Ordering::Relaxed);
        // A replay forked from a checkpoint listens once it is there, so
        // that the debugger finds it stopped.
        if stop.why == CHECKPOINT {
            crate::diagnostics::listen_again();
        }
    }
    state.held.push(stop);
    // Out of Tondo code while it waits, so collections and dumps go on.
    let paused = crate::tasks::pause_here();
    let resumed = state.resumed;
    let mut aside = false;
    // A frozen task stays while the others go on. What the debugger's thread
    // asks is looked at before each wait, with `state`, which it holds to ask:
    // the next request may come while the last one ends.
    while state.resumed == resumed || frozen(&state) {
        // The others go on: another worker takes this one's place. Only then,
        // since every task a stop holds would ask for one.
        if state.resumed != resumed && !aside {
            crate::tasks::step_aside();
            aside = true;
        }
        // Work the debugger's thread asks of this task, as showing a value
        // through a view.
        let work = WORK.lock().unwrap_or_else(|e| e.into_inner()).take_if(|(t, _)| *t == task);
        if let Some((_, work)) = work {
            drop(state);
            if paused {
                crate::tasks::enter_tondo();
            }
            work();
            if paused {
                crate::tasks::pause_here();
            }
            *WORKED.lock().unwrap_or_else(|e| e.into_inner()) = true;
            WORK_DONE.notify_all();
            state = self::state();
            continue;
        }
        // A call the debugger asks this task to make, on its own stack.
        let asked = CALL.lock().unwrap_or_else(|e| e.into_inner()).take_if(|(t, ..)| *t == task);
        if let Some((_, code, addresses)) = asked {
            drop(state);
            if paused {
                crate::tasks::enter_tondo();
            }
            let call: extern "C" fn(u64, u64) -> *mut Str = unsafe { std::mem::transmute(code as usize) };
            CALLING.store(task, Ordering::Relaxed);
            let made = call(addresses.as_ptr() as u64, 0);
            CALLING.store(0, Ordering::Relaxed);
            let text = (!made.is_null()).then(|| String::from_utf8_lossy(unsafe { str_bytes(made) }).into_owned());
            unsafe { crate::tondo_rt_str_release(made) };
            if paused {
                crate::tasks::pause_here();
            }
            *CALLED.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(text.ok_or_else(|| "the call panicked".to_string()));
            CALLED_DONE.notify_all();
            state = self::state();
            continue;
        }
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    state.held.retain(|held| held.task != task);
    drop(state);
    if aside {
        crate::tasks::step_back();
    }
    if paused {
        crate::tasks::enter_tondo();
    }
}

/// Whether a panic stops the program while a debugger is attached
/// (`catch panic`, the default, or `catch none`), and whether an error does
/// (`catch error`).
pub(super) static CATCH: AtomicBool = AtomicBool::new(true);
pub(super) static CATCH_ERRORS: AtomicBool = AtomicBool::new(false);
/// The panics `catch panic` names, by code or name, and the errors `catch
/// error` names, by type; none names, all.
pub(super) static CAUGHT: Mutex<(Vec<String>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));

/// Whether a panic's headline, `panic[P0001]: bounds: ...`, is of a code
/// or name `catch` named.
pub(super) fn caught_panic(headline: &str) -> bool {
    let wanted = &CAUGHT.lock().unwrap_or_else(|e| e.into_inner()).0;
    let Some((code, rest)) = headline.strip_prefix("panic[").and_then(|h| h.split_once("]: ")) else {
        return wanted.is_empty();
    };
    let name = rest.split(':').next().unwrap_or_default();
    wanted.is_empty() || wanted.iter().any(|w| w == code || w == name)
}

/// Whether an error of the type `ty`, shown as `shown`, is of one `catch`
/// named: by the type's name, with its module or without, or for a union,
/// by the member it is.
pub(super) fn caught_error(ty: &str, shown: &str) -> bool {
    let wanted = &CAUGHT.lock().unwrap_or_else(|e| e.into_inner()).1;
    let ty = match ty.contains(" | ") {
        true => shown.split_once(": ").map_or(ty, |(member, _)| member),
        false => ty,
    };
    let own = ty.rsplit('.').next().unwrap_or(ty);
    wanted.is_empty() || wanted.iter().any(|w| w == ty || w == own)
}

/// In a debug build, an error leaves a function where it starts: a `fail`
/// of the program's code, or a `?` that passes one on from the standard
/// library. With a debugger that catches errors, the task stops there, the
/// error as why, and the others at their next probe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_debug_failed(
    site: i64,
    addresses: *const usize,
    alive: *const u64,
    glue: Show,
    error: *const u8,
    ty: *const Str,
) {
    if !attached() || !CATCH_ERRORS.load(Ordering::Relaxed) {
        return;
    }
    if calling_here() {
        return;
    }
    let shown = show(glue, error as usize);
    if !caught_error(&String::from_utf8_lossy(unsafe { str_bytes(ty) }), &shown) {
        return;
    }
    let frame = unsafe { *(crate::trace::frame_pointer() as *const usize) };
    let task = crate::tasks::current_id();
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let (addresses, alive) = (addresses as usize, alive as usize);
    let stop = Stop { task, site: site as usize, frame, bounds, addresses, alive, why: String::new() };
    let why = format!("error: {}", clipped(&shown));
    hold(state(), stop, Some(why));
}

/// A task panics: with a debugger attached that catches panics, it stops
/// where the program's own code panicked, before it unwinds, the panic's
/// text as why, and the others at their next probe.
pub(crate) fn panicked(message: &str) {
    if calling_here() {
        return;
    }
    if !attached() || !CATCH.load(Ordering::Relaxed) || !caught_panic(message) {
        return;
    }
    stop_here(message.to_string());
}

/// A panic a `recover` will stop, which ends nothing: a breakpoint on that
/// stops where the program's code panicked.
pub(crate) fn recovering(message: &str) {
    if attached() && EVENTS.load(Ordering::Relaxed) & ON_RECOVER != 0 && !calling_here() {
        stop_here(message.to_string());
    }
}

/// The events breakpoints stop at (`break on ...`), a bit each, and the
/// least wait in milliseconds that stops: a task's start, a channel's send
/// or receive, a long wait.
pub(super) static EVENTS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub(super) static LONGEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
pub(super) const ON_SPAWN: u8 = 1;
pub(super) const ON_SEND: u8 = 2;
pub(super) const ON_RECEIVE: u8 = 4;
pub(super) const ON_WAIT: u8 = 8;
pub(super) const ON_LOCK: u8 = 16;
pub(super) const ON_REQUEST: u8 = 32;
pub(super) const ON_RECOVER: u8 = 64;

/// An event a breakpoint stops at.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Event {
    Spawn,
    Send,
    Receive,
    Wait(u64),
    Lock,
    Request,
    Recover,
}

/// The tasks that started while a breakpoint stops tasks as they start,
/// which stop at their first line; those that took a request, at their
/// first of the program's own, saying which.
pub(super) static STARTING: Mutex<Vec<(u64, Option<String>)>> = Mutex::new(Vec::new());
pub(super) static STARTED: AtomicBool = AtomicBool::new(false);

/// A task starts.
pub(crate) fn began(task: u64) {
    if EVENTS.load(Ordering::Relaxed) & ON_SPAWN != 0 {
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).push((task, None));
        STARTED.store(true, Ordering::Relaxed);
    }
}

/// A task sends on a channel, or receives from one: a breakpoint on that
/// stops it there, before.
pub(crate) fn channel_used(kind: u8, channel: usize) {
    let event = if kind == crate::tasks::SENDING { ON_SEND } else { ON_RECEIVE };
    if EVENTS.load(Ordering::Relaxed) & event == 0 || calling_here() {
        return;
    }
    let (count, capacity) = crate::tasks::channel_fill(channel);
    let filled = capacity.map_or(format!("{count}"), |capacity| format!("{count} of {capacity}"));
    let verb = if event == ON_SEND { "sends on" } else { "receives from" };
    stop_here(format!("{verb} a channel that holds {filled}"));
}

/// A task finds a lock taken: a breakpoint on that stops it, before it waits.
pub(crate) fn lock_taken(object: usize) {
    if !attached() || EVENTS.load(Ordering::Relaxed) & ON_LOCK == 0 || calling_here() {
        return;
    }
    let held: Vec<String> =
        crate::tasks::holders(object).iter().map(|(t, _, at)| format!("task {t} holds, taken at {at}")).collect();
    stop_here(format!("wants a std.sync lock {}", held.join(" and ")));
}

/// A task takes a request to answer: a breakpoint on that stops it where
/// the program's handler starts.
pub(crate) fn serving(method: &str, path: &str) {
    if attached() && EVENTS.load(Ordering::Relaxed) & ON_REQUEST != 0 {
        let told = Some(format!("serves {method} {path}"));
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).push((crate::tasks::current_id(), told));
        STARTED.store(true, Ordering::Relaxed);
    }
}

/// A task's wait ended after `waited`: a breakpoint on waits longer stops
/// it there.
pub(crate) fn waited(task: u64, waited: std::time::Duration) {
    let longest = LONGEST.load(Ordering::Relaxed);
    if waited.as_millis() < u128::from(longest) || calling_here() {
        return;
    }
    use crate::tasks::{JOIN, LOCK, RECEIVING, SENDING};
    let what = match crate::tasks::blocked_of(task) {
        Some((LOCK, _)) => "for a std.sync lock".to_string(),
        Some((RECEIVING, _)) => "to receive from a channel".to_string(),
        Some((SENDING, _)) => "to send on a channel".to_string(),
        Some((JOIN, object)) => {
            crate::tasks::joined(object).map_or("for a task to end".into(), |t| format!("for task {t} to end"))
        }
        _ => "in a wait".to_string(),
    };
    stop_here(format!("waited {} ms {what}", waited.as_millis()));
}

/// Whether the running task makes a call the debugger asked for, or runs
/// a view's code.
pub(super) fn calling_here() -> bool {
    let calling = CALLING.load(Ordering::Relaxed);
    (calling != 0 && calling == crate::tasks::current_id()) || SHOWING.get()
}

#[cfg_attr(not(all(tondo_staticlib, any(target_os = "linux", target_os = "macos"))), allow(dead_code))]
/// Stops the running task where it is for good: each `continue` stops it
/// there again. Without a debugger the program ends, saying why.
pub(crate) fn stop_for_good(why: String) -> ! {
    // Each stop holds the task until the debugger lets it go.
    loop {
        if !attached() || !stop_here(why.clone()) {
            break;
        }
    }
    // Straight to the descriptor: the program may be writing to its
    // standard error, whose lock it holds.
    let message = format!("tondo: {why}\n");
    unsafe { libc::write(2, message.as_ptr().cast(), message.len()) };
    std::process::exit(70);
}

/// Stops the running task, from inside the runtime, where the program's
/// own code called it, `why` as why, and the others at their next probe;
/// gives whether it did, which it cannot outside the program's code.
pub(super) fn stop_here(why: String) -> bool {
    let Some(table) = table() else { return false };
    let task = crate::tasks::current_id();
    let bounds = crate::tasks::stack_bounds().unwrap_or((0, 0));
    let mut found = None;
    for (frame, address) in crate::trace::callers(crate::trace::frame_pointer(), bounds) {
        let Some(described) = crate::trace::describe(address) else { continue };
        let Some((file, line)) = described.at else { continue };
        let function = table.functions.iter().position(|f| f.name == described.function);
        let Some(function) = function.filter(|_| !file.starts_with("<std>/") && frame != 0) else { continue };
        let (addressed, living) = &table.offsets[function];
        let (addressed, living) = (addressed.load(Ordering::Relaxed), living.load(Ordering::Relaxed));
        // The probe of the statement that panicked, where the stop shows.
        let site = table.sites.iter().enumerate().filter(|(_, s)| s.function == function && s.line <= line);
        let Some((site, _)) = site.max_by_key(|(_, s)| s.line) else { continue };
        if addressed == UNKNOWN {
            continue;
        }
        let (addresses, alive) = ((frame as isize + addressed) as usize, (frame as isize + living) as usize);
        found = Some(Stop { task, site, frame, bounds, addresses, alive, why: String::new() });
        break;
    }
    drop(table);
    let Some(stop) = found else { return false };
    hold(state(), stop, Some(why));
    true
}

/// What a task waits for, in words, and the tasks that keep it waiting:
/// those that hold the lock it wants, or the one it waits to finish.
pub(crate) fn waits_for(task: u64) -> Option<(String, Vec<u64>)> {
    use crate::tasks::{JOIN, LOCK, RECEIVING, SENDING};
    let (kind, object) = crate::tasks::blocked_of(task)?;
    let filled = |object: usize| {
        let (count, capacity) = crate::tasks::channel_fill(object);
        capacity.map_or(format!("{count}"), |capacity| format!("{count} of {capacity}"))
    };
    Some(match kind {
        LOCK => {
            let holders = crate::tasks::holders(object);
            let held: Vec<String> = holders.iter().map(|(t, _, at)| format!("task {t} holds, taken at {at}")).collect();
            let text = match held.is_empty() {
                true => "a std.sync lock".to_string(),
                false => format!("a std.sync lock {}", held.join(" and ")),
            };
            (text, holders.iter().map(|(t, ..)| *t).collect())
        }
        RECEIVING => (format!("a value of a channel that holds {}", filled(object)), Vec::new()),
        SENDING => (format!("room in a channel that holds {}", filled(object)), Vec::new()),
        JOIN => match crate::tasks::joined(object) {
            Some(joined) => (format!("task {joined} to end"), vec![joined]),
            None => ("a task to end".to_string(), Vec::new()),
        },
        _ => return None,
    })
}

/// A cycle of tasks that wait for each other, through locks and joins, in
/// words: a deadlock's cause.
pub(crate) fn cycle(tasks: &[u64]) -> Option<String> {
    // From the lowest task, so that the same cycle reads the same.
    let mut tasks = tasks.to_vec();
    tasks.sort_unstable();
    let edges: Vec<(u64, String, Vec<u64>)> =
        tasks.iter().filter_map(|&t| waits_for(t).map(|(text, on)| (t, text, on))).collect();
    for &(start, ..) in &edges {
        let mut path = vec![start];
        let mut at = start;
        while let Some((_, _, on)) = edges.iter().find(|(t, ..)| *t == at) {
            let Some(&next) = on.first() else { break };
            if next == start {
                let told: Vec<String> = path
                    .iter()
                    .filter_map(|t| edges.iter().find(|(e, ..)| e == t))
                    .map(|(t, text, _)| format!("task {t} waits for {text}"))
                    .collect();
                return Some(told.join("; "));
            }
            if path.contains(&next) {
                break;
            }
            path.push(next);
            at = next;
        }
    }
    None
}

/// Every task waits and none can wake the others (`P0013`): with a
/// debugger attached that catches it, the program stops first, where the
/// main task waits, so that `tasks`, `stack` and `locals` show what each
/// one waits for; it ends when the debugger goes on.
pub(crate) fn deadlocked(headline: &str) {
    if !attached() || !CATCH.load(Ordering::Relaxed) || !caught_panic(headline) {
        return;
    }
    let Some(table) = table() else { return };
    let waiting = crate::tasks::waiting();
    let Some(&(task, _, frame, bounds)) = waiting.first() else { return };
    // The tasks that wait for each other, when locks or joins tie them.
    let ids: Vec<u64> = waiting.iter().map(|(id, ..)| *id).collect();
    let headline = match cycle(&ids) {
        Some(cycle) => format!("{headline}: {cycle}"),
        None => headline.to_string(),
    };
    let headline = headline.as_str();
    let stop = Stop { task, site: WAITING, frame, bounds, addresses: 0, alive: 0, why: headline.to_string() };
    let at = frames(&table, &stop).first().map_or(String::new(), |f| f.place.clone());
    drop(table);
    let mut state = state();
    tell(&format!("{{\"stopped\":{{\"why\":{},\"task\":{task},\"at\":{}}}}}", quoted(headline), quoted(&at)));
    state.stopped = Some(stop);
    HOLD.store(true, Ordering::Relaxed);
    let resumed = state.resumed;
    while state.resumed == resumed {
        state = RESUMED.wait(state).unwrap_or_else(|e| e.into_inner());
    }
}

/// The frame the debugger asked to restart, and its task; and the function
/// whose new call stops at its first line.
pub(super) static RESTART_FRAME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub(super) static RESTART_TASK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(super) static RESTARTED: Mutex<String> = Mutex::new(String::new());

/// Whether the frame at a probe is the one to restart (1), or moves to new
/// code (2).
pub(super) fn restarting(frame: usize, task: u64, site: usize) -> u8 {
    let asked = RESTART_FRAME.load(Ordering::Relaxed);
    match asked != 0 && asked == frame && RESTART_TASK.load(Ordering::Relaxed) == task {
        true => 1,
        // The frame the debugger moves to another line.
        false if crate::live::going(task, frame, site as i64) => 2,
        false => crate::live::moving(site as i64),
    }
}

pub(super) fn stopped_event(stop: &Stop) -> String {
    format!(
        "{{\"stopped\":{{\"why\":{},\"task\":{},\"at\":{}{}}}}}",
        quoted(&stop.why),
        stop.task,
        quoted(&place(stop.site)),
        returned(stop)
    )
}

/// What a function returned in the step that stopped, as an answer's
/// field.
pub(super) fn returned(stop: &Stop) -> String {
    match RETURNED.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some((task, function, value)) if *task == stop.task && stop.why == "step" => {
            format!(",\"returned\":{{\"function\":{},\"value\":{}}}", quoted(function), quoted(value))
        }
        _ => String::new(),
    }
}

/// A probe's place: `file:line in function`.
pub(crate) fn place(site: usize) -> String {
    let Some(table) = table() else { return String::new() };
    place_in(&table, site)
}

pub(super) fn place_in(table: &Table, site: usize) -> String {
    let Some(at) = table.sites.get(site) else { return "the start of the program".into() };
    format!("{}:{} in {}", at.file, at.line, table.functions[at.function].name)
}

/// How the program goes on once the debugger has the answer that says it
/// runs: a program that ends at once would end before the answer went.
pub(super) static AFTER_ANSWER: Mutex<Option<Mode>> = Mutex::new(None);

pub(super) fn go_after_answer(mode: Mode) {
    *AFTER_ANSWER.lock().unwrap_or_else(|e| e.into_inner()) = Some(mode);
}

/// Called once an answer went to the debugger.
pub(crate) fn answered() {
    if let Some(event) = TELL_AFTER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        tell(&event);
    }
    let mode = AFTER_ANSWER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mode) = mode {
        resume(mode);
    }
}

/// Goes on as `mode` says.
pub(super) fn resume(mode: Mode) {
    if !matches!(mode, Mode::Restarted { .. }) {
        RESTART_FRAME.store(0, Ordering::Relaxed);
    }
    let stepping = match mode {
        Mode::Step { task, .. } | Mode::Next { task, .. } | Mode::Finish { task, .. } | Mode::Into { task, .. } => task,
        Mode::Run | Mode::Restarted { .. } => 0,
    };
    STEPPING.store(stepping, Ordering::Relaxed);
    *RETURNED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *MOVED_TO.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let mut state = state();
    state.mode = mode;
    state.stopped = None;
    state.selected = None;
    state.past = None;
    HOLD.store(false, Ordering::Relaxed);
    state.resumed += 1;
    RESUMED.notify_all();
}
