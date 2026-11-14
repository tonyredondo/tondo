//! What a debugger asks (docs/refactor.md, phase 6): its commands, read from
//! the diagnostics socket, and their answers in JSON.

use super::*;

/// Attaches the tool on `client` as the debugger: it hears when the
/// program stops.
pub(crate) fn attach(client: &UnixStream) -> String {
    let Some(table) = table() else {
        return "{\"error\":\"the program was not built with --debug\"}".into();
    };
    let mut slot = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return "{\"error\":\"another tool debugs this program\"}".into();
    }
    use std::os::fd::AsRawFd;
    *slot = client.try_clone().ok();
    CLIENT_FD.store(client.as_raw_fd(), Ordering::Relaxed);
    drop(slot);
    // A program that waits for the debugger stands still before `main`
    // from now on, until it says go on.
    if WAIT.load(Ordering::Relaxed) && BEFORE_MAIN.load(Ordering::Relaxed) {
        let stop =
            Stop { task: 1, site: usize::MAX, frame: 0, bounds: (0, 0), addresses: 0, alive: 0, why: "start".into() };
        tell(&stopped_event(&stop));
        state().stopped = Some(stop);
    }
    tools::listen(tools::DEBUGGER, true);
    format!("{{\"debugging\":true,\"lines\":{}}}", table.sites.len())
}

/// The debugger went away: the program goes on as if it never came.
pub(crate) fn detach() {
    // A replay without a debugger keeps no more checkpoints, nor does the
    // program start children for it.
    EVERY.store(0, Ordering::Relaxed);
    FOLLOW.store(false, Ordering::Relaxed);
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CLIENT_FD.store(-1, Ordering::Relaxed);
    tools::listen(tools::DEBUGGER, false);
    let mut state = state();
    state.breakpoints.clear();
    state.frozen.clear();
    FREEZING.store(false, Ordering::Relaxed);
    unwatch(&mut state, |_| true);
    drop(state);
    if let Some(table) = table() {
        table.marked.iter().for_each(|m| m.store(false, Ordering::Relaxed));
    }
    resume(Mode::Run);
}

/// Whether the connection on `fd` is the debugger's.
pub(crate) fn is_client(fd: i32) -> bool {
    CLIENT_FD.load(Ordering::Relaxed) == fd
}

/// The program ended: the debugger hears it.
pub(crate) fn exited() {
    // A scan to the end gives what it followed as the program ends.
    let following = TRACKED.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    let tracked = match (following, table()) {
        (true, Some(table)) => track(&table, "tracked", "").map_or(String::new(), |t| format!(",\"tracked\":{t}")),
        _ => String::new(),
    };
    tell(&format!("{{\"exited\":true{tracked}}}"));
}

/// The program crashed in a replay, as it did in its recording: a scan
/// gives what it followed all the same, which is what a crash is searched
/// for, unless the crash came with a lock this takes.
pub(crate) fn crashed() {
    let free = TRACKED.try_lock().is_ok() && CLIENT.try_lock().is_ok();
    if free && crate::replay::replaying() {
        exited();
    }
}

/// Answers a command of the debugger, or `None` for one it does not know.
pub(crate) fn command(line: &str) -> Option<String> {
    if line == "checkpoint" && crate::replay::replaying() {
        return Some(take_checkpoint());
    }
    if line == "sources" {
        return Some(format!("{{\"sources\":{}}}", quoted(&crate::diagnostics::roots())));
    }
    let _commanding = Commanding::enter();
    let table = table()?;
    let table = &*table;
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    let stopped = || state().stopped.clone();
    let not_stopped = || "{\"error\":\"the program runs: `pause` stops it, or a breakpoint\"}".to_string();
    Some(match word {
        "break" => match rest.strip_prefix("on ") {
            Some(event) => event_breakpoint(table, event),
            None => breakpoint(table, rest),
        },
        "delete" => {
            let id: u32 = rest.parse().unwrap_or(0);
            let mut state = state();
            // One that is gone, as a watch whose frame returned, is no
            // error to a client that removes what it set.
            let missing = !state.breakpoints.iter().any(|b| b.id == id) && !state.watches.iter().any(|w| w.id == id);
            state.breakpoints.retain(|b| b.id != id);
            unwatch(&mut state, |w| w.id == id);
            remark(table, &state);
            format!("{{\"deleted\":{id},\"missing\":{missing}}}")
        }
        // A program that runs goes on as it is: a stop that comes as this
        // is answered is new, and nothing asked it to go on from that one.
        "continue" => match stopped() {
            Some(_) => {
                go_after_answer(Mode::Run);
                "{\"running\":true}".into()
            }
            None => "{\"running\":true,\"already\":true}".into(),
        },
        // In a replay, the probes the program's thread passed (`tick`), a
        // run on to one of them (`seek <tick>`), or through to it keeping
        // where breakpoints would stop and where a task's lines start
        // (`scan <tick> [<task>]`), which `scanned` gives.
        "tick" if crate::replay::replaying() => format!("{{\"tick\":{}}}", crate::replay::ticks()),
        "seek" | "scan" if crate::replay::replaying() => {
            let mut words = rest.split_whitespace();
            let Some(tick) = words.next().and_then(|t| t.parse::<u64>().ok()) else {
                return Some(format!("{{\"error\":\"{word} takes a probe's tick, as `tick` gives\"}}"));
            };
            if tick <= crate::replay::ticks() {
                return Some(
                    "{\"error\":\"the replay is past that probe: a replay of the recording from its start goes back\"}"
                        .into(),
                );
            }
            let task = words.next().and_then(|t| t.parse().ok()).unwrap_or(0);
            *SCANNED.lock().unwrap_or_else(|e| e.into_inner()) = (task, Default::default(), Default::default());
            SCANNING.store(word == "scan", Ordering::Relaxed);
            AIM.store(tick, Ordering::Relaxed);
            go_after_answer(Mode::Run);
            "{\"running\":true}".into()
        }
        "checkpoints" if crate::replay::replaying() => {
            let every: u64 = rest.parse().unwrap_or(0);
            EVERY.store(every, Ordering::Relaxed);
            format!("{{\"checkpoints\":{every}}}")
        }
        "scanned" => {
            let scan = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
            let list = |ticks: &std::collections::VecDeque<u64>| {
                ticks.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
            };
            format!("{{\"hits\":[{}],\"lines\":[{}]}}", list(&scan.1), list(&scan.2))
        }
        "why" => match why(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "anchor" => match anchor(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "track" | "trail" | "count" | "tracked" => match track(table, word, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "effects" => match effects(table) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "timeline" => match timeline(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "ran" => match ran(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "causes" => match causes(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `until <line>` or `until <file>:<line>`: runs until that line, or
        // the next stop, wherever that is.
        "until" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let at = match rest.parse::<u32>() {
                Ok(line) => match table.sites.get(stop.site) {
                    Some(site) => format!("{}:{line}", site.file),
                    None => return Some("{\"error\":\"until takes a line of a file, as `until main.to:12`\"}".into()),
                },
                Err(_) => rest.to_string(),
            };
            let sites = match line_sites(table, &at) {
                Ok(sites) => sites,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let mut state = state();
            let id = state.next_breakpoint;
            state.next_breakpoint += 1;
            let task = None;
            state.breakpoints.push(Breakpoint {
                id,
                sites,
                request: None,
                condition: None,
                count: 0,
                hits: None,
                log: None,
                task,
                once: true,
                event: None,
                capture: false,
                unparsed: None,
                compiled: Vec::new(),
            });
            remark(table, &state);
            drop(state);
            go_after_answer(Mode::Run);
            "{\"running\":true}".into()
        }
        // The children the program starts, debugged too, or no longer.
        "follow" => {
            let on = rest.trim() != "off";
            FOLLOW.store(on, Ordering::Relaxed);
            format!("{{\"following\":{on}}}")
        }
        // The history: kept from now on, or no longer.
        "record" => {
            let lines = match rest {
                "off" => 0,
                "" => 10_000,
                lines => match lines.parse::<usize>() {
                    Ok(lines) if lines > 0 => lines,
                    _ => {
                        return Some(
                            "{\"error\":\"record takes how many lines it keeps, as `record 10000`, or `off`\"}".into(),
                        );
                    }
                },
            };
            RECORDING.store(lines, Ordering::Relaxed);
            if lines == 0 {
                HISTORY.lock().unwrap_or_else(|e| e.into_inner()).clear();
                state().past = None;
            }
            format!("{{\"recording\":{lines}}}")
        }
        "back" | "forward" | "reverse" => match moved(table, word, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // A step in the past goes forward in the history.
        "step" | "next" | "finish" if state().past.is_some() => match moved(table, "forward", "1") {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "restart" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let Some(at) = table.sites.get(stop.site) else {
                return Some("{\"error\":\"the program has not started: `continue` starts it\"}".into());
            };
            let function = table.functions[at.function].name.clone();
            if !crate::live::on() {
                return Some("{\"error\":\"a frame restarts in a live build: debug the program with --live\"}".into());
            }
            if function.starts_with("closure in ") {
                return Some(
                    "{\"error\":\"a closure's frame does not restart: step out of it with `finish`, and restart its caller\"}"
                        .into(),
                );
            }
            RESTART_TASK.store(stop.task, Ordering::Relaxed);
            RESTART_FRAME.store(stop.frame, Ordering::Relaxed);
            *RESTARTED.lock().unwrap_or_else(|e| e.into_inner()) = function;
            go_after_answer(Mode::Restarted { task: stop.task });
            "{\"running\":true}".into()
        }
        "step" | "next" | "finish" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            let in_std = table.sites.get(stop.site).is_some_and(Site::generated);
            STEPS_STD.store(STEPS_ALL.load(Ordering::Relaxed) || in_std, Ordering::Relaxed);
            let mode = match word {
                // A call of the line, by its function's name.
                "step" if rest.starts_with("into ") => {
                    *INTO.lock().unwrap_or_else(|e| e.into_inner()) = rest["into ".len()..].trim().to_string();
                    Mode::Into { task: stop.task, frame: stop.frame }
                }
                "step" => Mode::Step { task: stop.task, frame: stop.frame },
                "next" => Mode::Next { task: stop.task, frame: stop.frame },
                _ => Mode::Finish { task: stop.task, frame: stop.frame },
            };
            go_after_answer(mode);
            "{\"running\":true}".into()
        }
        "pause" => {
            // A replay on its way to a probe stops where it got to.
            AIM.store(u64::MAX, Ordering::Relaxed);
            SCANNING.store(false, Ordering::Relaxed);
            HOLD.store(true, Ordering::Relaxed);
            "{\"pausing\":true}".into()
        }
        "set" => match set(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `goto <line> <move>`: the stopped frame goes on at the statement
        // of that line of its function, by the move `tondo` worked out from
        // the live build's points, as soon as it goes on; until then it
        // shows there.
        "goto" => {
            let Some(stop) = stopped() else { return Some(not_stopped()) };
            if !crate::live::on() {
                return Some(
                    "{\"error\":\"a frame goes to another line in a live build: debug the program with --live\"}"
                        .into(),
                );
            }
            let (line, hex) = rest.split_once(' ').unwrap_or((rest, ""));
            let bytes: Option<Vec<u8>> = (0..hex.len())
                .step_by(2)
                .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
                .collect();
            let (Ok(line), Some(bytes)) = (line.parse::<u32>(), bytes) else {
                return Some("{\"error\":\"goto takes a line and a move as tondo writes it\"}".into());
            };
            if let Err(message) = crate::live::goto(stop.task, stop.frame, stop.site as i64, &bytes) {
                return Some(format!("{{\"error\":{}}}", quoted(&message)));
            }
            *MOVED_TO.lock().unwrap_or_else(|e| e.into_inner()) = Some((stop.task, line));
            let at = moved_place(&place_in(table, stop.site), line);
            *TELL_AFTER.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("{{\"stopped\":{{\"why\":\"goto\",\"task\":{},\"at\":{}}}}}", stop.task, quoted(&at)));
            // The stop at the target line comes next, as after a step.
            format!("{{\"running\":true,\"goto\":{line}}}")
        }
        // What a call of the program's code gives, on the stopped task.
        "call" => match called(table, rest) {
            Ok(text) => format!("{{\"called\":{}}}", quoted(&text)),
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `freeze <task>`: it stays held, at its next line if it runs, while
        // the others go on; `thaw <task>` lets it go on.
        "freeze" | "thaw" => {
            let Ok(task) = rest.parse::<u64>() else {
                return Some(format!(
                    "{{\"error\":{}}}",
                    quoted(&format!("{word} takes a task's number, as `tasks` lists them"))
                ));
            };
            let mut state = state();
            state.frozen.retain(|&t| t != task);
            if word == "freeze" {
                state.frozen.push(task);
            }
            FREEZING.store(!state.frozen.is_empty(), Ordering::Relaxed);
            let frozen: Vec<String> = state.frozen.iter().map(u64::to_string).collect();
            drop(state);
            RESUMED.notify_all();
            format!("{{\"frozen\":[{}]}}", frozen.join(","))
        }
        "retainers" => match retainers(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // Where the code of each frame of a task is at.
        "pcs" => {
            let state = state();
            let task = rest.parse::<u64>().ok().or(state.stopped.as_ref().map(|s| s.task));
            let held = task.and_then(|task| stop_of(&state, task));
            let code = state.code.iter().find(|&&(at, _)| Some(at) == task).map(|&(_, code)| code);
            drop(state);
            let Some(held) = held else { return Some(not_stopped()) };
            let frames = frames(table, &held);
            let pc = |(index, frame): (usize, &Frame)| match (index, code) {
                (0, Some(code)) => code,
                _ => pc_of(table, frame),
            };
            let pcs: Vec<String> = frames.iter().enumerate().map(|at| pc(at).to_string()).collect();
            format!("{{\"pcs\":[{}]}}", pcs.join(","))
        }
        // Where the bytes of each binding of the frame looked at are, or
        // of a part of one.
        "memories" if state().past.is_none() => match selected(table) {
            Ok((frames, index)) if rest.is_empty() => {
                let visible = visible(table, &frames, index);
                let memory = |(b, at, _): &(&Binding, usize, _)| {
                    format!("{}:{}", quoted(&b.name), memory_of(table, b.layout, *at))
                };
                let at: Vec<String> = visible.iter().map(memory).collect();
                format!("{{\"memories\":{{{}}}}}", at.join(","))
            }
            Ok((frames, index)) => match located(table, &frames, index, rest, false) {
                Ok((at, layout, _)) => {
                    format!("{{\"memories\":{{{}:{}}}}}", quoted(rest), memory_of(table, layout, at))
                }
                Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
            },
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "peek" => match rest.split_once(' ').and_then(|(at, count)| Some((at.parse().ok()?, count.parse().ok()?))) {
            Some((at, count)) if state().past.is_none() => {
                let bytes = peek(at, usize::min(count, MOST_PEEKED));
                format!("{{\"bytes\":\"{}\"}}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
            }
            Some(_) => {
                "{\"error\":\"the history keeps values, not memory: go back to where the program stopped\"}".into()
            }
            None => "{\"error\":\"peek takes an address and a count\"}".into(),
        },
        "poke" => match poked(rest) {
            Ok(written) => format!("{{\"written\":{written}}}"),
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "code" => match rest.parse().ok().and_then(crate::trace::code_at) {
            Some(code) => {
                let line = |&(at, file, line): &(usize, &str, u32)| format!("[{at},{},{line}]", quoted(file));
                let lines: Vec<String> = code.lines.iter().map(line).collect();
                format!(
                    "{{\"function\":{},\"start\":{},\"end\":{},\"lines\":[{}]}}",
                    quoted(code.function),
                    code.start,
                    code.end,
                    lines.join(",")
                )
            }
            None => "{\"error\":\"no function of the program is there\"}".into(),
        },
        "memory" => match memory() {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // `bindings <breakpoint>`: the file of its lines, and the bindings
        // its condition reads there with their types, for `tondo` to
        // compile it.
        "bindings" => {
            let id: u32 = rest.parse().unwrap_or(0);
            let state = state();
            let Some(breakpoint) = state.breakpoints.iter().find(|b| b.id == id) else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("no breakpoint {id}"))));
            };
            let Some(&first) = breakpoint.sites.first() else {
                return Some("{\"error\":\"the breakpoint is at no line\"}".into());
            };
            let functions: std::collections::BTreeSet<usize> =
                breakpoint.sites.iter().map(|&s| table.sites[s].function).collect();
            let mut bindings: Vec<(&str, &str)> = Vec::new();
            for function in functions {
                let mut name = table.functions[function].name.as_str();
                let mut around = vec![function];
                while let Some((outer, false)) = written_in(name) {
                    let Some(found) = table.functions.iter().position(|f| f.name == outer) else { break };
                    around.push(found);
                    name = outer;
                }
                bindings.extend(around.iter().flat_map(|&f| &table.functions[f].bindings).map(|b| (&*b.name, &*b.ty)));
            }
            // A name bound with two types, which the patch could not tell
            // apart, stays out: a condition that reads it is evaluated.
            let mut out: Vec<String> = Vec::new();
            for (index, &(name, ty)) in bindings.iter().enumerate() {
                let earliest = bindings.iter().position(|(n, _)| *n == name) == Some(index);
                if earliest && bindings.iter().all(|(n, t)| *n != name || *t == ty) {
                    out.push(format!("{{\"name\":{},\"type\":{}}}", quoted(name), quoted(ty)));
                }
            }
            format!("{{\"file\":{},\"bindings\":[{}]}}", quoted(&table.sites[first].file), out.join(","))
        }
        // The conditions only compiled code reads whose breakpoints have
        // lines of code they were not compiled for, after a reload.
        "conditions" => {
            let state = state();
            let pending = state.breakpoints.iter().filter_map(|b| {
                let text = b.unparsed.as_ref()?;
                let known = |s: &usize| b.compiled.iter().any(|(.., sites)| sites.contains(s));
                (!b.sites.iter().all(known)).then(|| format!("{{\"id\":{},\"condition\":{}}}", b.id, quoted(text)))
            });
            format!("{{\"conditions\":[{}]}}", pending.collect::<Vec<_>>().join(","))
        }
        // `condition <breakpoint> <slot> <binding>...`: its condition as a
        // live patch compiled it, which the breakpoint calls from now on.
        "condition" => {
            let mut words = rest.split_whitespace();
            let (Some(id), Some(slot)) =
                (words.next().and_then(|w| w.parse::<u32>().ok()), words.next().and_then(|w| w.parse().ok()))
            else {
                return Some("{\"error\":\"condition takes a breakpoint, a slot and the bindings it reads\"}".into());
            };
            let Some(code) = crate::live::slot_code(slot).filter(|&c| c != 0) else {
                return Some("{\"error\":\"the program is not a live build\"}".into());
            };
            let names = words.map(String::from).collect();
            let mut state = state();
            match state.breakpoints.iter_mut().find(|b| b.id == id) {
                Some(breakpoint) => {
                    let known: Vec<usize> = breakpoint.compiled.iter().flat_map(|(.., s)| s.clone()).collect();
                    let sites = breakpoint.sites.iter().filter(|s| !known.contains(s)).copied().collect();
                    breakpoint.compiled.push((code, names, sites));
                    format!("{{\"condition\":{{\"id\":{id},\"compiled\":true}}}}")
                }
                None => format!("{{\"error\":{}}}", quoted(&format!("no breakpoint {id}"))),
            }
        }
        // Whether steps go into the standard library's code.
        "steps" => match rest {
            "all" | "program" => {
                STEPS_ALL.store(rest == "all", Ordering::Relaxed);
                format!("{{\"steps\":{}}}", quoted(rest))
            }
            _ => "{\"error\":\"steps takes `program`, to step over the standard library's code, or `all`\"}".into(),
        },
        // An expression the frame looked at watches, until it returns.
        "watch" => match watch(table, rest) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        // What stops the program: panics, errors, both or none.
        // `catch panic [<code or name>...] error [<type>...]`, or `none`.
        "catch" => {
            let (mut panics, mut errors): (Option<Vec<String>>, Option<Vec<String>>) = (None, None);
            let mut last = None;
            for word in rest.split_whitespace() {
                match (word, last) {
                    ("panic", _) => (panics, last) = (Some(Vec::new()), Some(true)),
                    ("error", _) => (errors, last) = (Some(Vec::new()), Some(false)),
                    ("none", None) => last = Some(true),
                    (name, Some(true)) if panics.is_some() => panics.get_or_insert_default().push(name.into()),
                    (name, Some(false)) => errors.get_or_insert_default().push(name.into()),
                    _ => {
                        let message = "catch takes `panic` and its codes or names, as `panic bounds P0005`, to stop where \
                            a task panics, `error` and its types, as `error ConfigError`, where an error starts, or `none`";
                        return Some(format!("{{\"error\":{}}}", quoted(message)));
                    }
                }
            }
            if last.is_none() {
                return Some("{\"error\":\"catch takes `panic`, `error`, both, or `none`\"}".into());
            }
            CATCH.store(panics.is_some(), Ordering::Relaxed);
            CATCH_ERRORS.store(errors.is_some(), Ordering::Relaxed);
            let kinds: Vec<String> = [("panic", &panics), ("error", &errors)]
                .iter()
                .filter_map(|(kind, names)| {
                    names.as_ref().map(|names| quoted(format!("{kind} {}", names.join(" ")).trim_end()))
                })
                .collect();
            *CAUGHT.lock().unwrap_or_else(|e| e.into_inner()) =
                (panics.unwrap_or_default(), errors.unwrap_or_default());
            format!("{{\"catch\":[{}]}}", kinds.join(","))
        }
        "select" => {
            // A task and its frame, or a frame of the task looked at.
            let numbers: Vec<Option<u64>> = rest.split_whitespace().map(|w| w.parse().ok()).collect();
            let mut state = state();
            let current = state.selected.map(|(task, _)| task).or(state.stopped.as_ref().map(|s| s.task));
            let (task, frame) = match numbers[..] {
                [Some(task), Some(frame)] => (task, frame),
                [Some(frame)] if current.is_some() => (current.unwrap_or(0), frame),
                _ => {
                    return Some(
                        "{\"error\":\"select takes a frame, or a task and its frame, as `select 1 0`\"}".into(),
                    );
                }
            };
            let Some(held) = stop_of(&state, task) else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} is not stopped"))));
            };
            let count = frames(table, &held).len() as u64;
            if frame >= count {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} has {count} frames"))));
            }
            state.selected = Some((task, frame as usize));
            format!("{{\"selected\":{{\"task\":{task},\"frame\":{frame}}}}}")
        }
        // All of a value, which `print` cuts short.
        "whole" => {
            let (frames, index) = match selected(table) {
                Ok(selected) => selected,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let found = expression::parse(rest).and_then(|e| evaluating(table, &frames, index, |ev| ev.evaluate(&e)));
            match found {
                Ok(evaluated) => format!("{{\"whole\":{}}}", quoted(&evaluated.text)),
                Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
            }
        }
        // `cancel <task>`: it stops at its next wait, once it runs.
        "cancel" => match rest.parse::<u64>() {
            Ok(task) => {
                crate::tasks::cancel_by_id(task);
                format!("{{\"cancelled\":{task}}}")
            }
            Err(_) => "{\"error\":\"cancel takes a task's number, as `tasks` lists them\"}".to_string(),
        },
        // `lines <from> <to> <file>`: where in those lines a breakpoint stops.
        "lines" => {
            let mut words = rest.splitn(3, ' ');
            let number = |word: Option<&str>| word.and_then(|w| w.parse::<u32>().ok()).unwrap_or(0);
            let (from, to, file) = (number(words.next()), number(words.next()), words.next().unwrap_or_default());
            let in_file = |site: &&Site| file == site.file || file.ends_with(&format!("/{}", site.file));
            let mut lines: Vec<u32> =
                table.sites.iter().filter(in_file).filter(|s| !s.goes_on).map(|s| s.line).collect();
            lines.retain(|line| (from..=to).contains(line));
            lines.sort_unstable();
            lines.dedup();
            format!("{{\"lines\":{lines:?}}}")
        }
        // The files of the program's code.
        "files" => {
            let mut files: Vec<&str> = table.sites.iter().map(|site| site.file.as_str()).collect();
            files.sort_unstable();
            files.dedup();
            format!("{{\"files\":[{}]}}", files.iter().map(|f| quoted(f)).collect::<Vec<_>>().join(","))
        }
        "locals" | "print" | "children" if state().past.is_some() => {
            let state = state();
            let frame = state.selected.map_or(0, |(_, frame)| frame);
            let Some((past, history)) = past(&state) else { return Some(not_stopped()) };
            in_past(&history[past], word, rest, frame)
        }
        "locals" | "print" | "children" => {
            let (frames, index) = match selected(table) {
                Ok(selected) => selected,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            if word == "locals" {
                let frame = &frames[index];
                if frame.addresses == 0 && frame.function.is_some() {
                    let message =
                        "this frame's bindings are not known: none of its function's lines ran since the debugger came";
                    return Some(format!("{{\"error\":{}}}", quoted(message)));
                }
                let looked = {
                    let state = state();
                    state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0)))
                };
                let items = locals_of(table, &frames, index, looked.map_or(0, |(task, _)| task));
                // In the frame a step stopped in, what a function returned.
                let state = state();
                let returned = match &state.stopped {
                    Some(stop) if looked == Some((stop.task, 0)) => returned(stop),
                    _ => String::new(),
                };
                return Some(format!("{{\"locals\":[{}]{returned}}}", items.join(",")));
            }
            if word == "print" {
                return Some(printed(table, &frames, index, rest));
            }
            // The parts of a value by its path, all or a page of them:
            // `children <path> [<start> <count>]`.
            let (path, page) = match rest.rsplitn(3, ' ').collect::<Vec<_>>()[..] {
                [count, start, path] => match (start.parse::<usize>(), count.parse::<usize>()) {
                    (Ok(start), Ok(count)) => (path.trim(), Some((start, count))),
                    _ => (rest, None),
                },
                _ => (rest, None),
            };
            let found = match resolve(table, &frames, index, path) {
                Ok(found) => found,
                Err(message) => return Some(format!("{{\"error\":{}}}", quoted(&message))),
            };
            let answer = children_of(path, &found.text, page);
            // A viewed binding's value as it is comes first, as `raw()`.
            let bound = visible(table, &frames, index);
            let viewed = bound.iter().find(|(b, ..)| b.name == path && b.raw.is_some());
            match viewed.and_then(|(binding, address, _)| Some(show(binding.raw?, *address))) {
                Some(raw) => {
                    let item = format!(
                        "{{\"name\":\"raw()\",\"path\":{},{}}}",
                        quoted(&format!("{path}.raw()")),
                        described(&raw)
                    );
                    let more = if answer.starts_with("{\"children\":[]") { "" } else { "," };
                    answer.replacen("{\"children\":[", &format!("{{\"children\":[{item}{more}"), 1)
                }
                None => answer,
            }
        }
        // The contexts of the task looked at, and which `with` set each
        // field (language 11.16).
        "context" => {
            let state = state();
            let task = state.selected.map(|(task, _)| task).or(state.stopped.as_ref().map(|s| s.task));
            drop(state);
            let Some(task) = task.and_then(crate::tasks::by_id) else { return Some(not_stopped()) };
            let contexts: Vec<String> = unsafe { crate::contexts::described(task) }
                .into_iter()
                .map(|context| {
                    let fields: Vec<String> = context
                        .fields
                        .iter()
                        .map(|field| {
                            let (at, inherited) = match &field.set {
                                Some((at, inherited)) => (quoted(at), *inherited),
                                None => ("null".to_string(), false),
                            };
                            format!(
                                "{{\"name\":{},\"value\":{},\"children\":{},\"set\":{at},\"inherited\":{inherited}}}",
                                quoted(&field.name),
                                quoted(&clipped(&field.value)),
                                field.parts
                            )
                        })
                        .collect();
                    format!("{{\"name\":{},\"fields\":[{}]}}", quoted(&context.name), fields.join(","))
                })
                .collect();
            format!("{{\"contexts\":[{}]}}", contexts.join(","))
        }
        "stack" if state().past.is_some() => {
            let state = state();
            let Some((past, history)) = past(&state) else { return Some(not_stopped()) };
            let moment = &history[past];
            // Another task than the one whose history it shows is as now.
            if rest.parse::<u64>().is_ok_and(|task| task != moment.task) {
                let task: u64 = rest.parse().unwrap_or(0);
                let Some(held) = stop_of(&state, task) else {
                    return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} is not stopped"))));
                };
                let frames: Vec<String> = frames(table, &held).iter().map(|f| quoted(&f.place)).collect();
                return Some(format!("{{\"stack\":[{}]}}", frames.join(",")));
            }
            let frames: Vec<String> = moment.stack.iter().map(|f| quoted(f)).collect();
            format!("{{\"stack\":[{}]}}", frames.join(","))
        }
        "stack" => {
            let state = state();
            let task = match rest.parse::<u64>() {
                Ok(task) => Some(task),
                Err(_) => state.stopped.as_ref().map(|s| s.task),
            };
            let Some(task) = task else { return Some(not_stopped()) };
            let held = stop_of(&state, task);
            drop(state);
            let Some(held) = held else {
                return Some(format!("{{\"error\":{}}}", quoted(&format!("task {task} is not stopped"))));
            };
            let mut frames: Vec<String> = frames(table, &held).iter().map(|f| f.place.clone()).collect();
            let spawned: Vec<String> = spawned(task).iter().map(|s| quoted(s)).collect();
            // A frame `goto` moves shows where it goes on.
            if let (Some((_, line)), Some(first)) =
                (MOVED_TO.lock().unwrap_or_else(|e| e.into_inner()).filter(|(t, _)| *t == task), frames.first_mut())
            {
                *first = moved_place(first, line);
            }
            let frames: Vec<String> = frames.iter().map(|f| quoted(f)).collect();
            // Where the task was spawned, and its spawner: the stack's
            // cause.
            let spawned = match spawned.is_empty() {
                true => String::new(),
                false => format!(",\"spawned\":[{}]", spawned.join(",")),
            };
            format!("{{\"stack\":[{}]{spawned}}}", frames.join(","))
        }
        "snapshot" => match snapshot(table) {
            Ok(answer) => answer,
            Err(message) => format!("{{\"error\":{}}}", quoted(&message)),
        },
        "tasks" => {
            let state = state();
            let held: Vec<String> = state
                .held
                .iter()
                .map(|held| {
                    let why = match &state.stopped {
                        Some(stop) if stop.task == held.task => stop.why.clone(),
                        _ if state.frozen.contains(&held.task) => "frozen".to_string(),
                        _ => "held".to_string(),
                    };
                    format!(
                        "{{\"task\":{},\"at\":{},\"why\":{}{}}}",
                        held.task,
                        quoted(&place_in(table, held.site)),
                        quoted(&why),
                        request_field(held.task)
                    )
                })
                .collect();
            // While the program stands still, the tasks that wait, or are
            // ready to run again, too: where in the program each is.
            let mut waiting = Vec::new();
            if state.stopped.is_some() {
                for (task, ..) in crate::tasks::waiting() {
                    let Some(stop) = stop_of(&state, task).filter(|s| s.site == WAITING) else { continue };
                    let at = frames(table, &stop).first().map_or(String::new(), |f| f.place.clone());
                    let (at, why) = (quoted(&at), quoted(&stop.why));
                    // What it waits for, and the tasks that hold that.
                    let waits = waits_for(task).map_or(String::new(), |(text, on)| {
                        let on: Vec<String> = on.iter().map(u64::to_string).collect();
                        let ms = crate::tasks::blocked_for(task);
                        format!(",\"for\":{},\"on\":[{}],\"ms\":{ms}", quoted(&text), on.join(","))
                    });
                    let serving = request_field(task);
                    waiting.push(format!("{{\"task\":{task},\"at\":{at},\"why\":{why}{waits}{serving}}}"));
                }
            }
            format!("{{\"tasks\":[{}]}}", held.into_iter().chain(waiting).collect::<Vec<_>>().join(","))
        }
        _ => return None,
    })
}

/// `watch <expression> [if <condition>] [hits <count>]`, in the frame
/// looked at: of a binding that holds a `std.sync` cell, or a part of its
/// value, from every task.
pub(super) fn watch(table: &Table, text: &str) -> Result<String, String> {
    let (text, hits) = match text.split_once(" hits ") {
        Some((text, hits)) => (text, Some(Hits::parse(hits)?)),
        None => (text, None),
    };
    let (text, condition) = match text.split_once(" if ") {
        Some((text, condition)) => {
            (text.trim(), Some(expression::parse(condition).map_err(|m| format!("the condition: {m}"))?))
        }
        None => (text.trim(), None),
    };
    let (frames, index) = selected(table)?;
    let expr = expression::parse(text)?;
    let evaluated = evaluating(table, &frames, index, |e| e.evaluate(&expr))?;
    // A binding of a cell, as `hits` in `hits.value`.
    let root = evaluated.path.as_deref().map(|path| path.split(['.', '[']).next().unwrap_or(path));
    let bound = visible(table, &frames, index);
    let shared = root.and_then(|root| bound.iter().find(|(b, ..)| b.name == root)).and_then(|(binding, address, _)| {
        matches!(table.layouts[binding.layout], Layout::Shared).then(|| {
            let cell = unsafe { *(*address as *const usize) };
            unsafe { crate::sync::retain(cell as *mut crate::sync::SyncObject) };
            (cell, binding.name.clone(), binding.show)
        })
    });
    let mut state = state();
    let (task, _) = state
        .selected
        .or(state.stopped.as_ref().map(|s| (s.task, 0)))
        .ok_or("the program runs: `pause` stops it, or a breakpoint")?;
    let last = state.held.iter().filter(|h| h.task == task).map(|h| (h.frame, h.site)).collect();
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    let everywhere = if shared.is_some() { ",\"everywhere\":true" } else { "" };
    let answer = format!("{{\"watch\":{{\"id\":{id},{}{everywhere}}}}}", described(&evaluated.text));
    let (frames, value) = (frames[index..].to_vec(), evaluated.text);
    let text = text.to_string();
    state.watches.push(Watch { id, task, expr, text, frames, value, last, condition, count: 0, hits, shared });
    WATCHING.store(true, Ordering::Relaxed);
    Ok(answer)
}

/// `set <target> [=] <value>`: changes a binding of the frame where the
/// task stopped, or a part of one, `user.age` or `items[i]`, to what the
/// expression gives: a number, a `Bool`, a `Char`, a `String` or a plain
/// enum's variant, as `Role.Admin`. An array's item changes when no other
/// value shares the array. Gives what the target is now.
pub(super) fn set(table: &Table, text: &str) -> Result<String, String> {
    let usage =
        || "set takes what changes and its new value, as `set count count + 1` or `set user.age = 37`".to_string();
    // The target ends at a space outside brackets and quotes.
    let (mut depth, mut quoted) = (0i32, false);
    let space = text.char_indices().find_map(|(at, c)| {
        match c {
            '"' => quoted = !quoted,
            '[' | '(' if !quoted => depth += 1,
            ']' | ')' if !quoted => depth -= 1,
            ' ' if !quoted && depth == 0 => return Some(at),
            _ => {}
        }
        None
    });
    let (target, value) = text.split_once(" = ").or_else(|| space.map(|at| text.split_at(at))).ok_or_else(usage)?;
    let (target, value) = (target.trim(), value.trim());
    if state().selected.is_some_and(|(_, frame)| frame != 0) {
        return Err("a binding changes in the frame where its task stopped: `frame 0`".into());
    }
    let (frames, index) = selected(table)?;
    let (address, layout, ty) = located(table, &frames, index, target, true)?;
    let evaluated = evaluating(table, &frames, index, |e| e.evaluate(&expression::parse(value)?));
    match (&table.layouts[layout], evaluated) {
        (Layout::Scalar(ty), Ok(evaluated)) => unsafe { write(ty, address, &evaluated.value)? },
        // A text without its quotes, as it was written before.
        (Layout::Scalar(ty), Err(_)) if ty == "String" => unsafe {
            write(ty, address, &expression::Value::Text(value.to_string()))?
        },
        (Layout::Plain(size, variants), Ok(evaluated)) => {
            let shown = evaluated.value.shown();
            let name = shown.rsplit('.').next().unwrap_or_default();
            let Some(tag) = variants.iter().position(|v| v == name) else {
                return Err(format!("`{value}` is none of its variants: {}", variants.join(", ")));
            };
            unsafe { std::ptr::copy_nonoverlapping(&(tag as u64).to_le_bytes()[0], address as *mut u8, *size) };
        }
        (_, Err(message)) => return Err(message),
        _ => return Err(format!("the debugger does not change `{target}`, a value of type {ty}")),
    }
    Ok(printed(table, &frames, index, target))
}

/// An object of the heap as `retainers` and `memory` name it: its size,
/// where it was made, and a text's first characters.
pub(super) fn object_named(objects: &crate::heap::Objects, at: usize) -> String {
    let (_, size, site, text) = &objects.list[at];
    let made = made_at(*site);
    match text {
        Some(text) => format!("the text {:?} of {size} bytes, made at {made}", clip(text, 40)),
        None => format!("an object of {size} bytes, made at {made}"),
    }
}

pub(super) fn made_at(site: usize) -> String {
    crate::trace::describe(site)
        .and_then(|d| d.at.map(|(file, line)| format!("{file}:{line} in {}", d.function)))
        .unwrap_or_else(|| "a place the debugger does not know".into())
}

pub(super) fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text.to_string(),
    }
}

/// `retainers <path>`: what keeps the value a binding or a part of one
/// holds alive, when it is an object of the heap: the stacks of the tasks
/// that point to it, and the shortest chains of objects from a stack to it.
pub(super) fn retainers(table: &Table, target: &str) -> Result<String, String> {
    let (frames, index) = selected(table)?;
    let (address, ..) = located(table, &frames, index, target, false)?;
    let object = unsafe { *(address as *const usize) };
    let objects = crate::heap::objects().ok_or("the program does not keep its objects: a debug build does")?;
    let Ok(found) = objects.list.binary_search_by_key(&object, |o| o.0) else {
        return Err(format!("`{target}` is not an object of the heap: its value is in its frame, or in another value"));
    };
    let mut by: Vec<Vec<usize>> = vec![Vec::new(); objects.list.len()];
    for &(from, to) in &objects.edges {
        by[to].push(from);
    }
    let stacks = |at: usize| -> Vec<u64> { objects.roots.iter().filter(|r| r.1 == at).map(|r| r.0).collect() };
    // From the object back to the stacks, the nearest first.
    let mut came: Vec<Option<usize>> = vec![None; objects.list.len()];
    let mut seen = vec![false; objects.list.len()];
    let mut queue = std::collections::VecDeque::from([found]);
    seen[found] = true;
    let mut chains = Vec::new();
    while let Some(at) = queue.pop_front() {
        if at != found && !stacks(at).is_empty() {
            let mut chain = vec![format!("the stack of task {}", stacks(at)[0])];
            let mut step = Some(at);
            while let Some(here) = step {
                chain.push(object_named(&objects, here));
                step = came[here];
            }
            chains.push(format!("[{}]", chain.iter().map(|c| quoted(c)).collect::<Vec<_>>().join(",")));
            if chains.len() == 3 {
                break;
            }
        }
        for &from in &by[at] {
            if !seen[from] {
                seen[from] = true;
                came[from] = Some(at);
                queue.push_back(from);
            }
        }
    }
    let held: Vec<String> = stacks(found).iter().map(u64::to_string).collect();
    Ok(format!(
        "{{\"retainers\":{{\"object\":{},\"stacks\":[{}],\"chains\":[{}]}}}}",
        quoted(&object_named(&objects, found)),
        held.join(","),
        chains.join(",")
    ))
}

/// What `memory` counted last time, by where the objects were made.
pub(super) static COUNTED: Mutex<Vec<(usize, usize, usize)>> = Mutex::new(Vec::new());

/// The most `peek` reads at once.
pub(super) const MOST_PEEKED: usize = 1 << 20;

/// A piece of memory goes through at most this much of a pipe at once, and
/// never over the end of a page.
pub(super) const PIECE: usize = 4096;

/// A pipe, closed with this: its two ends.
pub(super) struct Pipe([i32; 2]);

impl Pipe {
    pub(super) fn new() -> Option<Pipe> {
        let mut ends = [0; 2];
        (unsafe { libc::pipe(ends.as_mut_ptr()) } == 0).then_some(Pipe(ends))
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0[0]);
            libc::close(self.0[1]);
        }
    }
}

/// The length of the piece that starts at `at`, of what remains.
pub(super) fn piece(at: usize, remaining: usize) -> usize {
    usize::min(PIECE - at % PIECE, remaining)
}

/// The bytes at `address`, up to the first that cannot be read. They go
/// through a pipe, since the system says that memory is not there where
/// reading it would crash the program.
pub(super) fn peek(address: usize, count: usize) -> Vec<u8> {
    let _outside = crate::replay::Outside::enter();
    let mut bytes = vec![0u8; count];
    let Some(pipe) = Pipe::new() else { return Vec::new() };
    let mut done = 0;
    while done < count {
        let at = address.wrapping_add(done);
        let length = piece(at, count - done);
        if unsafe { libc::write(pipe.0[1], at as *const libc::c_void, length) } != length as isize {
            break;
        }
        unsafe { libc::read(pipe.0[0], bytes[done..].as_mut_ptr().cast(), length) };
        done += length;
    }
    bytes.truncate(done);
    bytes
}

/// `poke <address> <hex>`: writes bytes, through a pipe as `peek` reads
/// them; how many it could.
pub(super) fn poked(rest: &str) -> Result<usize, String> {
    if crate::replay::replaying() {
        return Err("a replay is the run as it was: its memory does not change".into());
    }
    if state().past.is_some() {
        return Err("the history keeps values, not memory: go back to where the program stopped".into());
    }
    let (address, hex) = rest.split_once(' ').ok_or("poke takes an address and bytes in hexadecimal")?;
    let address: usize = address.parse().map_err(|_| "poke takes an address and bytes in hexadecimal")?;
    let pairs = hex.as_bytes().chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok());
    let bytes: Vec<u8> = pairs.collect::<Option<_>>().ok_or("poke takes an address and bytes in hexadecimal")?;
    let _outside = crate::replay::Outside::enter();
    let pipe = Pipe::new().ok_or("no pipe to write through")?;
    let mut done = 0;
    while done < bytes.len() {
        let at = address.wrapping_add(done);
        let length = piece(at, bytes.len() - done);
        unsafe { libc::write(pipe.0[1], bytes[done..].as_ptr().cast(), length) };
        if unsafe { libc::read(pipe.0[0], at as *mut libc::c_void, length) } != length as isize {
            break;
        }
        done += length;
    }
    Ok(done)
}

/// The address of the code a frame is at: a stopped frame, where its line
/// starts, since a stop is there.
pub(super) fn pc_of(table: &Table, frame: &Frame) -> usize {
    let line = || frame.place.split(" in ").next()?.rsplit_once(':')?.1.parse().ok();
    match (frame.pc, frame.function, line()) {
        (0, Some(function), Some(line)) => crate::trace::line_start(&table.functions[function].name, line).unwrap_or(0),
        (pc, ..) => pc,
    }
}

/// Where the bytes of a value at `address` are: there for a number or a
/// record, and where it points for one of the heap.
pub(super) fn memory_of(table: &Table, layout: usize, address: usize) -> usize {
    let inline = match &table.layouts[layout] {
        Layout::Scalar(ty) => ty != "String",
        Layout::Fields(_) | Layout::Plain(..) => true,
        Layout::Array(..) | Layout::Shared | Layout::Other => false,
    };
    let word = (!inline).then(|| peek(address, 8)).filter(|word| word.len() == 8);
    word.map_or(address, |word| usize::from_ne_bytes(word.try_into().unwrap()))
}

/// `memory`: the program's objects by where they were made, the places
/// with the most bytes first, and how each grew since `memory` last
/// counted.
pub(super) fn memory() -> Result<String, String> {
    let objects = crate::heap::objects().ok_or("the program does not keep its objects: a debug build does")?;
    let mut by: std::collections::HashMap<usize, (usize, usize)> = std::collections::HashMap::new();
    for &(_, size, site, _) in &objects.list {
        let place = by.entry(site).or_default();
        place.0 += 1;
        place.1 += size;
    }
    let mut counted = COUNTED.lock().unwrap_or_else(|e| e.into_inner());
    let before = std::mem::take(&mut *counted);
    let mut places: Vec<(usize, (usize, usize))> = by.into_iter().collect();
    places.sort_by_key(|&(site, (_, bytes))| (std::cmp::Reverse(bytes), site));
    *counted = places.iter().map(|&(site, (count, bytes))| (site, count, bytes)).collect();
    let rows: Vec<String> = places
        .iter()
        .take(20)
        .map(|&(site, (count, bytes))| {
            let grew = match before.iter().find(|b| b.0 == site) {
                Some(&(_, was, was_bytes)) => (count as i64 - was as i64, bytes as i64 - was_bytes as i64),
                None if before.is_empty() => (0, 0),
                None => (count as i64, bytes as i64),
            };
            format!(
                "{{\"at\":{},\"objects\":{count},\"bytes\":{bytes},\"more\":{},\"more_bytes\":{}}}",
                quoted(&made_at(site)),
                grew.0,
                grew.1
            )
        })
        .collect();
    let (count, bytes) = (objects.list.len(), objects.list.iter().map(|o| o.1).sum::<usize>());
    Ok(format!("{{\"memory\":{{\"objects\":{count},\"bytes\":{bytes},\"places\":[{}]}}}}", rows.join(",")))
}

/// Where a binding of a frame, or a part of one, as `user.age` or
/// `items[i]`, is in memory, its layout and the binding's type; `writing`
/// first copies an array another value shares, as a change does.
pub(super) fn located(
    table: &Table,
    frames: &[Frame],
    index: usize,
    target: &str,
    writing: bool,
) -> Result<(usize, usize, String), String> {
    let mut steps = Vec::new();
    let mut place = &expression::parse(target)?;
    let root = loop {
        match place {
            Expr::Name(name) => break name.clone(),
            Expr::Field(inner, field) => {
                steps.push(Ok(field.clone()));
                place = inner;
            }
            Expr::Index(inner, index) => {
                steps.push(Err(&**index));
                place = inner;
            }
            _ => return Err(format!("`{target}` is not a binding or a part of one")),
        }
    };
    let bound = bound(table, &frames[index]);
    let Some(&(binding, mut address)) = bound.iter().find(|(b, _)| b.name == root) else {
        return Err(format!("no binding `{root}` holds a value here"));
    };
    let mut layout = binding.layout;
    for step in steps.into_iter().rev() {
        match (step, &table.layouts[layout]) {
            (Ok(field), Layout::Fields(fields)) => {
                let Some(&(_, offset, boxed, inner)) = fields.iter().find(|(name, ..)| *name == field) else {
                    return Err(format!("`{target}` has no field `{field}`"));
                };
                address += offset;
                if boxed {
                    address = unsafe { *(address as *const usize) };
                }
                layout = inner;
            }
            (Err(at), Layout::Array(size, inner)) => {
                let at = match evaluating(table, frames, index, |e| e.evaluate(at))?.value {
                    expression::Value::Int(at) => at,
                    other => return Err(format!("an index is an Int, not {}", other.shown())),
                };
                let slot = address as *mut *mut crate::ArrayHeader;
                let length = if unsafe { *slot }.is_null() { 0 } else { unsafe { (**slot).len } };
                let Some(at) = usize::try_from(at).ok().filter(|&at| at < length) else {
                    return Err(format!("{at} is not an index of `{target}`, which has {length} items"));
                };
                // An array another value shares is copied first, as the
                // program does, when its items hold nothing counted.
                if writing && !unsafe { crate::unique(*slot) } {
                    let plain = match &table.layouts[*inner] {
                        Layout::Scalar(ty) => ty != "String",
                        Layout::Plain(..) => true,
                        _ => false,
                    };
                    if !plain {
                        let message = "another value shares this array, which a change here would change too";
                        return Err(format!("{message}: change the binding that holds it"));
                    }
                    unsafe { crate::tondo_rt_array_unique(slot, *size, 0, None, None) };
                }
                address = unsafe { crate::element(*slot, *size, at) } as usize;
                layout = *inner;
            }
            _ if writing => {
                return Err(format!(
                    "the debugger does not change `{target}`: it changes numbers, Bool, Char, String and plain enums, in records, tuples and arrays"
                ));
            }
            _ => return Err(format!("the debugger finds the parts of records, tuples and arrays, not `{target}`")),
        }
    }
    Ok((address, layout, binding.ty.clone()))
}

/// Writes a number, a `Bool`, a `Char` or a `String` of the type `ty` at
/// `address`.
pub(super) unsafe fn write(ty: &str, address: usize, value: &expression::Value) -> Result<(), String> {
    use expression::Value;
    let wrong = || format!("{} is not a value of type {ty}", value.shown());
    macro_rules! int {
        ($t:ty) => {{
            let Value::Int(n) = value else { return Err(wrong()) };
            let n = <$t>::try_from(*n).map_err(|_| wrong())?;
            unsafe { (address as *mut $t).write_unaligned(n) };
        }};
    }
    macro_rules! float {
        ($t:ty) => {{
            let x = match value {
                Value::Float(x) => *x,
                Value::Int(n) => *n as f64,
                _ => return Err(wrong()),
            };
            unsafe { (address as *mut $t).write_unaligned(x as $t) };
        }};
    }
    match (ty, value) {
        ("Int", _) => int!(i64),
        ("Int32", _) => int!(i32),
        ("Int16", _) => int!(i16),
        ("Int8", _) => int!(i8),
        ("Int128", _) => int!(i128),
        ("UInt", _) => int!(u64),
        ("UInt128", _) => int!(u128),
        ("UInt32", _) => int!(u32),
        ("UInt16", _) => int!(u16),
        ("UInt8" | "Byte", _) => int!(u8),
        ("Float", _) => float!(f64),
        ("Float32", _) => float!(f32),
        ("Bool", Value::Bool(b)) => unsafe { *(address as *mut u8) = *b as u8 },
        ("Char", Value::Char(c)) => unsafe { *(address as *mut u32) = *c as u32 },
        ("String", Value::Text(text)) => {
            let old = unsafe { *(address as *const *mut Str) };
            unsafe { *(address as *mut *mut Str) = crate::str_new(text.as_bytes()) };
            unsafe { crate::tondo_rt_str_release(old) };
        }
        _ => return Err(wrong()),
    }
    Ok(())
}

/// Where a task the debugger looks at is: held at a probe, or, while the
/// program stands still, waiting.
pub(super) fn stop_of(state: &State, task: u64) -> Option<Stop> {
    if let Some(held) = state.held.iter().chain(&state.stopped).find(|h| h.task == task) {
        return Some(held.clone());
    }
    state.stopped.as_ref()?;
    let (_, why, frame, bounds) = crate::tasks::waiting().into_iter().find(|(id, ..)| *id == task)?;
    Some(Stop { task, site: WAITING, frame, bounds, addresses: 0, alive: 0, why: why.into() })
}

/// The frame `locals` and `print` look at: the one `select` chose, or the
/// stopped task's own.
pub(super) fn selected(table: &Table) -> Result<(Vec<Frame>, usize), String> {
    let state = state();
    let not = "the program runs: `pause` stops it, or a breakpoint".to_string();
    let (task, index) = state.selected.or(state.stopped.as_ref().map(|s| (s.task, 0))).ok_or(not.clone())?;
    let held = stop_of(&state, task).ok_or(not)?;
    drop(state);
    let frames = frames(table, &held);
    match index < frames.len() {
        true => Ok((frames, index)),
        false => Err(format!("task {task} has no frame {index}")),
    }
}

/// The function that the function `name` is written in, by how the
/// compiler names it: a closure's or a cleanup's, and for a test or a
/// suite, any suite, those around it being the ones on its stack.
pub(super) fn written_in(name: &str) -> Option<(&str, bool)> {
    if name.starts_with("test ") || name.starts_with("suite ") {
        return Some(("suite ", true));
    }
    name.strip_prefix("closure in ").or(name.strip_prefix("defer in ")).map(|outer| (outer, false))
}

/// The bindings the code of a frame can use, with where each is: its own,
/// then those of the functions it is written in whose frames are further
/// down its stack, as a closure's maker or a test's suites, each with the
/// function it is from.
pub(super) fn visible<'t>(
    table: &'t Table,
    frames: &[Frame],
    index: usize,
) -> Vec<(&'t Binding, usize, Option<&'t str>)> {
    let name = |frame: &Frame| frame.function.map(|f| table.functions[f].name.as_str());
    let mut out: Vec<(&Binding, usize, Option<&str>)> =
        bound(table, &frames[index]).into_iter().map(|(b, a)| (b, a, None)).collect();
    let mut at = index;
    while let Some((outer, prefix)) = name(&frames[at]).and_then(written_in) {
        let found = (at + 1..frames.len()).find(|&below| {
            name(&frames[below]).is_some_and(|n| if prefix { n.starts_with(outer) } else { n == outer })
        });
        let Some(below) = found else { break };
        let from = name(&frames[below]);
        for (binding, address) in bound(table, &frames[below]) {
            if !out.iter().any(|(b, ..)| b.name == binding.name) {
                out.push((binding, address, from));
            }
        }
        at = below;
    }
    out
}

/// `break <file>:<line> [if <condition>] [hits <count>] [log <message>]`:
/// the probes of that line, or of the next line that has one in the file;
/// `break fn <name> ...`, those where each function of that name starts.
pub(super) fn breakpoint(table: &Table, text: &str) -> String {
    let error = |message: String| format!("{{\"error\":{}}}", quoted(&message));
    let (text, log) = match text.split_once(" log ") {
        Some((text, log)) => (text, Some(log.to_string())),
        None => (text, None),
    };
    // `capture`, last: what it takes, without stopping.
    let (text, capture) = match text.strip_suffix(" capture") {
        Some(text) => (text, true),
        None => (text, false),
    };
    let (text, hits) = match text.split_once(" hits ") {
        Some((text, hits)) => match Hits::parse(hits) {
            Ok(hits) => (text, Some(hits)),
            Err(message) => return error(message),
        },
        None => (text, None),
    };
    let (at, condition) = match text.split_once(" if ") {
        Some((at, condition)) => (at.trim(), Some(condition.trim())),
        None => (text.trim(), None),
    };
    // One request's, `request <text>`.
    let (at, request) = match at.split_once(" request ") {
        Some((at, request)) => (at.trim(), Some(request.trim().to_string())),
        None => (at, None),
    };
    // One task's, `task <id>`.
    let (at, task) = match at.split_once(" task ") {
        Some((at, task)) => match task.trim().parse::<u64>() {
            Ok(task) => (at.trim(), Some(task)),
            Err(_) => return error(format!("`task` takes a task's number, as `tasks` lists them, not `{task}`")),
        },
        None => (at, None),
    };
    let sites = match at.strip_prefix("fn ") {
        Some(name) => function_sites(table, name.trim()),
        None => line_sites(table, at),
    };
    let sites = match sites {
        Ok(sites) => sites,
        Err(message) => return error(message),
    };
    // In a live build, `tondo` compiles the condition: one the debugger
    // does not read, as a call, stops nowhere until it does.
    let written = condition.filter(|_| crate::live::on());
    let (condition, unparsed) = match condition.map(expression::parse) {
        Some(Err(message)) if written.is_some() => (None, Some(message)),
        Some(Err(message)) if condition.is_some_and(|c| c.contains('(')) => {
            return error(format!(
                "the condition: {message}; a live build, debugged with --live, compiles a condition that calls the program's functions"
            ));
        }
        Some(Err(message)) => return error(format!("the condition: {message}")),
        Some(Ok(condition)) => (Some(condition), None),
        None => (None, None),
    };
    // What the condition and the message read are bindings there.
    let mut exprs: Vec<&Expr> = condition.iter().collect();
    let pieces = log.as_deref().map(pieces).unwrap_or_default();
    for (_, expr) in &pieces {
        match expr {
            Some(Ok(expr)) => exprs.push(expr),
            Some(Err(message)) => return error(format!("the message: {message}")),
            None => {}
        }
    }
    let functions: std::collections::BTreeSet<usize> = sites.iter().map(|&s| table.sites[s].function).collect();
    let usable: Vec<&str> = functions.into_iter().flat_map(|f| usable(table, f)).collect();
    let mut read = Vec::new();
    exprs.iter().for_each(|expr| expression::names(expr, &mut read));
    if let Some(name) = read.iter().find(|n| expression::binding_name(n) && !usable.contains(&n.as_str())) {
        let place = place_in(table, sites[0]);
        return error(format!("`{name}` is not a binding at {place}; these are: {}", usable.join(", ")));
    }
    let mut state = state();
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    let at = &table.sites[sites[0]];
    // Its condition, which `tondo` compiles in a live build.
    let shown = match (written, &unparsed) {
        (Some(c), Some(why)) => format!(",\"condition\":{},\"unparsed\":{}", quoted(c), quoted(why)),
        (Some(c), None) => format!(",\"condition\":{}", quoted(c)),
        (None, _) => String::new(),
    };
    let answer =
        format!("{{\"breakpoint\":{{\"id\":{id},\"file\":{},\"line\":{}{shown}}}}}", quoted(&at.file), at.line);
    let (event, compiled) = (None, Vec::new());
    let unparsed = unparsed.and(written).map(String::from);
    let breakpoint = Breakpoint {
        id,
        sites,
        condition,
        unparsed,
        count: 0,
        hits,
        log,
        task,
        request,
        once: false,
        event,
        capture,
        compiled,
    };
    state.breakpoints.push(breakpoint);
    remark(table, &state);
    answer
}

/// The probes of a line, `file:line`, or of the next line with code.
pub(super) fn line_sites(table: &Table, at: &str) -> Result<Vec<usize>, String> {
    let Some((file, line)) = at.rsplit_once(':') else {
        return Err(format!("a breakpoint is `file:line`, as `main.to:12`, or `fn name`, not `{at}`"));
    };
    let Ok(line) = line.parse::<u32>() else { return Err(format!("`{line}` is not a line number")) };
    // A file as the compiler named it, or a path that ends with it, as an
    // editor gives.
    let matches = |site: &Site| {
        site.file == file || site.file.ends_with(&format!("/{file}")) || file.ends_with(&format!("/{}", site.file))
    };
    let Some(found) = table.sites.iter().filter(|s| matches(s) && s.line >= line).map(|s| s.line).min() else {
        let files: std::collections::BTreeSet<&str> = table.sites.iter().map(|s| s.file.as_str()).collect();
        return Err(format!(
            "no code at {file}:{line} or after it; the program's files are {}",
            files.into_iter().collect::<Vec<_>>().join(", ")
        ));
    };
    let starts = |site: &Site| matches(site) && site.line == found && !site.goes_on;
    Ok(table.sites.iter().enumerate().filter(|(_, s)| starts(s)).map(|(i, _)| i).collect())
}

/// The probes where the functions called `name` start: by their whole
/// name, as `Stack.push`, or by their own, `push`; a generic one in each
/// of its types.
pub(super) fn function_sites(table: &Table, name: &str) -> Result<Vec<usize>, String> {
    let mut sites = Vec::new();
    for (index, _) in table.functions.iter().enumerate().filter(|(_, f)| named(f, name, false)) {
        let of = || table.sites.iter().enumerate().filter(|(_, s)| s.function == index);
        let Some(first) = of().map(|(_, s)| s.line).min() else { continue };
        sites.extend(of().filter(|(_, s)| s.line == first).map(|(i, _)| i));
    }
    if sites.is_empty() {
        let near: Vec<&str> = table
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .filter(|f| f.to_lowercase().contains(&name.to_lowercase()) && !f.starts_with("closure in "))
            .take(10)
            .collect();
        return Err(match near.is_empty() {
            true => format!("no function `{name}` has code in the program"),
            false => format!("no function `{name}` has code in the program; these do: {}", near.join(", ")),
        });
    }
    Ok(sites)
}

/// The names of the bindings the code of a function can use: its own, and
/// those of the functions it is written in.
pub(super) fn usable(table: &Table, function: usize) -> Vec<&str> {
    let mut out: Vec<&str> = table.functions[function].bindings.iter().map(|b| b.name.as_str()).collect();
    let mut name = table.functions[function].name.as_str();
    while let Some((outer, prefix)) = written_in(name) {
        let around =
            table.functions.iter().filter(|f| if prefix { f.name.starts_with(outer) } else { f.name == outer });
        let around: Vec<&Function> = around.collect();
        out.extend(around.iter().flat_map(|f| f.bindings.iter().map(|b| b.name.as_str())));
        match around.first() {
            Some(first) if !prefix => name = first.name.as_str(),
            _ => break,
        }
    }
    out
}

/// The bindings the code of a frame can use, as `locals` gives them: each
/// one changed since `locals` last showed that frame says so.
pub(super) fn locals_of(table: &Table, frames: &[Frame], index: usize, task: u64) -> Vec<String> {
    let key = (task, frames[index].pointer, frames[index].function);
    let shown: Vec<(String, String)> = visible(table, frames, index)
        .into_iter()
        .map(|(binding, address, _)| (binding.name.clone(), show(binding.show, address)))
        .collect();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let before = seen.iter().find(|(k, _)| *k == key).map(|(_, texts)| texts.clone());
    let items = visible(table, frames, index)
        .into_iter()
        .zip(&shown)
        .map(|((binding, _, from), (_, whole))| {
            let mut from = from.map_or(String::new(), |from| format!(",\"from\":{}", quoted(from)));
            // A view opens to its value as it is, too.
            if binding.raw.is_some() {
                from.push_str(",\"viewed\":true");
            }
            let was = before.as_ref().and_then(|b| b.iter().find(|(n, _)| *n == binding.name)).map(|(_, t)| t);
            let changed = if was.is_some_and(|was| was != whole) { ",\"changed\":true" } else { "" };
            format!(
                "{{\"name\":{},\"type\":{},{}{from}{changed}}}",
                quoted(&binding.name),
                quoted(&binding.ty),
                described(whole)
            )
        })
        .collect();
    seen.retain(|(k, _)| *k != key);
    seen.push((key, shown));
    // A few frames' worth: those a session looks at.
    if seen.len() > 64 {
        seen.remove(0);
    }
    items
}

/// What `locals` last showed of each frame, by its task, frame pointer and
/// function, which the next shows changes against.
#[allow(clippy::type_complexity)]
pub(super) static SEEN: Mutex<Vec<((u64, usize, Option<usize>), Vec<(String, String)>)>> = Mutex::new(Vec::new());

/// Where a task was spawned, and its spawner, as `file:line in function`:
/// what its stack goes on with.
pub(super) fn spawned(task: u64) -> Vec<String> {
    crate::tasks::spawned_at(task)
        .into_iter()
        .filter_map(|address| {
            let described = crate::trace::describe(address)?;
            let (file, line) = described.at?;
            (!described.function.starts_with("spawn")).then(|| format!("{file}:{line} in {}", described.function))
        })
        .collect()
}

/// `snapshot`: what a model needs of a stop in one answer: why and where
/// the task stopped, each frame of its stack with its bindings, where it
/// was spawned, the other tasks, and the last lines of its history.
pub(super) fn snapshot(table: &Table) -> Result<String, String> {
    let state = state();
    let stop = state.stopped.clone().ok_or("the program runs: `pause` stops it, or a breakpoint")?;
    let held = stop_of(&state, stop.task).unwrap_or(stop.clone());
    drop(state);
    let frames = frames(table, &held);
    let described: Vec<String> = (0..frames.len())
        .map(|index| {
            let locals = match frames[index].addresses == 0 && frames[index].function.is_some() {
                true => String::new(),
                false => locals_of(table, &frames, index, stop.task).join(","),
            };
            format!("{{\"at\":{},\"locals\":[{locals}]}}", quoted(&frames[index].place))
        })
        .collect();
    let spawned: Vec<String> = spawned(stop.task).iter().map(|s| quoted(s)).collect();
    let tasks = command("tasks").unwrap_or_default();
    let tasks = tasks.strip_prefix("{\"tasks\":").and_then(|t| t.strip_suffix('}')).unwrap_or("[]").to_string();
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mut lines: Vec<String> = history
        .iter()
        .rev()
        .filter(|m| m.task == stop.task)
        .skip(1)
        .take(10)
        .map(|m| quoted(&place_in(table, m.site)))
        .collect();
    lines.reverse();
    Ok(format!(
        "{{\"snapshot\":{{\"task\":{},\"why\":{},\"frames\":[{}],\"spawned\":[{}],\"tasks\":{tasks},\"before\":[{}]{}}}}}",
        stop.task,
        quoted(&stop.why),
        described.join(","),
        spawned.join(","),
        lines.join(","),
        returned(&stop)
    ))
}

/// `print <expression>`: its value in the frame looked at, with its type
/// when it has one, and the path that names it when it is a binding or a
/// part of one, whose parts `children` gives. A path that is not an
/// expression, as a map's key that is a record, reads as a path.
pub(super) fn printed(table: &Table, frames: &[Frame], index: usize, text: &str) -> String {
    let expr = match expression::parse(text) {
        Ok(expr) => expr,
        Err(message) => {
            return match resolve(table, frames, index, text) {
                Ok(found) => {
                    let ty = found.ty.as_deref().map_or("null".to_string(), quoted);
                    format!(
                        "{{\"name\":{},\"type\":{ty},{},\"path\":{}}}",
                        quoted(text),
                        described(&found.text),
                        quoted(text)
                    )
                }
                Err(_) => format!("{{\"error\":{}}}", quoted(&message)),
            };
        }
    };
    let evaluated = match evaluating(table, frames, index, |e| e.evaluate(&expr)) {
        Ok(evaluated) => evaluated,
        Err(message) => return format!("{{\"error\":{}}}", quoted(&message)),
    };
    let ty = match (&expr, &evaluated.path) {
        (Expr::Name(name), Some(_)) => {
            visible(table, frames, index).iter().find(|(b, ..)| b.name == *name).map(|(b, ..)| b.ty.clone())
        }
        (_, Some(_)) => None,
        (_, None) => evaluated.value.ty().map(str::to_string),
    };
    let ty = ty.as_deref().map_or("null".to_string(), quoted);
    let path = evaluated.path.as_deref().map_or(String::new(), |path| format!(",\"path\":{}", quoted(path)));
    format!("{{\"name\":{},\"type\":{ty},{}{path}}}", quoted(text), described(&evaluated.text))
}

/// Marks the sites that have a breakpoint, and the events some stop at.
pub(super) fn remark(table: &Table, state: &State) {
    for (site, marked) in table.marked.iter().enumerate() {
        marked.store(state.breakpoints.iter().any(|b| b.sites.contains(&site)), Ordering::Relaxed);
    }
    let (mut events, mut longest) = (0, u64::MAX);
    for event in state.breakpoints.iter().filter_map(|b| b.event) {
        events |= match event {
            Event::Spawn => ON_SPAWN,
            Event::Send => ON_SEND,
            Event::Receive => ON_RECEIVE,
            Event::Lock => ON_LOCK,
            Event::Request => ON_REQUEST,
            Event::Recover => ON_RECOVER,
            Event::Wait(ms) => {
                longest = longest.min(ms);
                ON_WAIT
            }
        };
    }
    EVENTS.store(events, Ordering::Relaxed);
    LONGEST.store(longest, Ordering::Relaxed);
    // Tasks that started stop no more once no breakpoint asks.
    if events & (ON_SPAWN | ON_REQUEST) == 0 {
        STARTING.lock().unwrap_or_else(|e| e.into_inner()).clear();
        STARTED.store(false, Ordering::Relaxed);
    }
}

/// `break on spawn|send|receive|lock|request|recover|wait <ms>`: a
/// breakpoint at an event.
pub(super) fn event_breakpoint(table: &Table, text: &str) -> String {
    let event = match text.split_whitespace().collect::<Vec<_>>()[..] {
        ["spawn"] => Event::Spawn,
        ["send"] => Event::Send,
        ["receive"] => Event::Receive,
        ["lock"] => Event::Lock,
        ["request"] => Event::Request,
        ["recover"] => Event::Recover,
        ["wait", ms] if ms.parse::<u64>().is_ok() => Event::Wait(ms.parse().unwrap_or(0)),
        _ => {
            let message = "break on takes `spawn`, `send`, `receive`, `lock`, `request`, `recover` or `wait <ms>`, as `break on wait 100`";
            return format!("{{\"error\":{}}}", quoted(message));
        }
    };
    let mut state = state();
    let id = state.next_breakpoint;
    state.next_breakpoint += 1;
    let (sites, condition, hits, log, task) = (Vec::new(), None, None, None, None);
    let event = Some(event);
    let (capture, compiled, unparsed) = (false, Vec::new(), None);
    let breakpoint = Breakpoint {
        id,
        sites,
        condition,
        unparsed,
        count: 0,
        hits,
        log,
        task,
        request: None,
        once: false,
        event,
        capture,
        compiled,
    };
    state.breakpoints.push(breakpoint);
    remark(table, &state);
    format!("{{\"breakpoint\":{{\"id\":{id},\"on\":{}}}}}", quoted(text.trim()))
}

pub(super) fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
