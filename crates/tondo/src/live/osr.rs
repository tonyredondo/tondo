//! How a patch moves frames to the new code (toolchain 7.10): a frame of
//! a function whose code changes goes, at the next statement it starts,
//! to the same statement of the new code, the one on the same line once
//! the lines that stay the same are matched, when what the new code needs
//! there comes from what the old has.

use serde_json::Value;
use std::collections::HashMap;

/// The moves a patch ends with, as the runtime reads them, how many, and
/// where frames stay on the old code and why.
pub(super) struct Moves {
    pub bytes: Vec<u8>,
    pub count: usize,
    pub stays: Vec<String>,
    /// The same by file and line, for an editor to mark.
    pub stay_lines: Vec<Value>,
    /// The entries the code lacks for frames to move to it, which another
    /// compilation adds; and the bindings the entries it has run again.
    pub wanted: super::CatchUps,
    pub again: Vec<String>,
}

/// An identity without what changes when only the shapes of the types its
/// signature reaches, or the number of its function's closures, do: its
/// name, parameters and result.
pub(super) fn base(identity: &str) -> String {
    let identity = identity.split(" @").next().unwrap_or(identity);
    match identity.find('(') {
        Some(at) => format!("{}{}", shown(identity), &identity[at..]),
        None => identity.to_string(),
    }
}

/// The moves from the points of `olds`, the running program's versions of
/// its functions, to those of `new`, by identity, for the functions
/// `changed`, whose slots are those of `slots`; values of types of another
/// shape go through the patch's conversions.
pub(super) fn moves(
    olds: &[&Value],
    new: &Value,
    changed: &[String],
    slots: &HashMap<String, u32>,
    migrations: &Migrations,
    renamed: &HashMap<String, String>,
    made: &super::CatchUps,
) -> Moves {
    let mut entries = Vec::new();
    let mut stays = Vec::new();
    let mut stay_lines = Vec::new();
    let mut wanted = super::CatchUps::new();
    let mut again = Vec::new();
    // Frames move to the same function, or to the one a function whose
    // signature reaches a type of another shape is now, of the same name
    // and signature, with their parameters converted.
    let mut pairs: Vec<(&Value, &str, Call)> = olds
        .iter()
        .flat_map(|old| changed.iter().filter_map(|identity| Some((old.get(identity.as_str())?, identity.as_str()))))
        .map(|(before, identity)| (before, identity, Call::default()))
        .collect();
    let names: Vec<&str> = new.as_object().into_iter().flatten().map(|(identity, _)| shown(identity)).collect();
    let mut redirects = Vec::new();
    for (identity, entry) in olds.iter().flat_map(|old| old.as_object().into_iter().flatten()) {
        let sited = entry["points"].as_array().is_some_and(|p| p.iter().any(|p| p["site"].is_i64()));
        if !sited || new.get(identity).is_some() || !names.contains(&shown(identity)) {
            continue;
        }
        let targets: Vec<&String> = new
            .as_object()
            .into_iter()
            .flatten()
            .map(|(now, _)| now)
            .filter(|now| *now != identity && base(now) == with_names(&base(identity), renamed))
            .collect();
        // A closure is the one of its function whose text is most its own.
        let closure = identity.starts_with("closure in ");
        let target = match targets.as_slice() {
            [target] if !closure => Some(*target),
            _ if closure => likeliest(entry, &targets, new),
            _ => None,
        };
        let found = match target {
            Some(target) => parameters(entry, &new[target.as_str()], migrations).map(|plan| (target, plan)),
            None if closure => Err("its function's closures changed, and none of them now is it".to_string()),
            None => Err("its parameters or result changed".to_string()),
        };
        match found {
            Ok((target, plan)) => {
                // Values of a closure that is only numbered otherwise now,
                // with captures of the same shape, run its new code.
                let same = |key: &str| entry[key] == new[target.as_str()][key];
                if closure && same("params") && same("result") {
                    redirects.extend(slots.get(identity).zip(slots.get(target)).map(|(&a, &b)| (a, b)));
                }
                pairs.push((entry, target.as_str(), plan))
            }
            Err(why) => stays.push(format!("frames of `{}` finish on the old code: {why}", shown(identity))),
        }
    }
    for (before, target, passed) in pairs {
        let (Some(after), Some(&slot)) = (new.get(target), slots.get(target)) else {
            continue;
        };
        let now = olds.first().and_then(|now| now.as_object());
        let current = now.is_some_and(|now| now.values().any(|code| std::ptr::eq(code, before)));
        let lines = line_map(&hashes(before), &hashes(after));
        let (from, to) = (list(before, "points"), list(after, "points"));
        // Each point by its line, what it is and its place among those of
        // its kind on that line.
        let place = |list: &[Value]| -> Vec<(u64, String, usize)> {
            let mut seen: HashMap<(u64, String), usize> = HashMap::new();
            list.iter()
                .map(|p| {
                    let key = (p["line"].as_u64().unwrap_or(0), p["kind"].as_str().unwrap_or_default().to_string());
                    let count = seen.entry(key.clone()).or_insert(0);
                    *count += 1;
                    (key.0, key.1, *count - 1)
                })
                .collect()
        };
        let (old_places, new_places) = (place(from), place(to));
        let at: HashMap<&(u64, String, usize), usize> = new_places.iter().enumerate().map(|(i, p)| (p, i)).collect();
        // A line changed in place may hold another kind of statement.
        let first = |line: u64| new_places.iter().position(|(on, kind, _)| *on == line && kind != "entry");
        let pairs: Vec<Option<usize>> = old_places
            .iter()
            .map(|(line, kind, ordinal)| {
                let line = *lines.get(&(*line as usize))? as u64;
                at.get(&(line, kind.clone(), *ordinal)).copied().or_else(|| first(line).filter(|_| *ordinal == 0))
            })
            .collect();
        // The statements of the old code by those of the new, which name
        // the values the compiler makes for them.
        let mut paths: HashMap<&str, &str> = HashMap::from([("", "")]);
        for (index, pair) in pairs.iter().enumerate() {
            if let Some(pair) = pair {
                paths.insert(
                    from[index]["path"].as_str().unwrap_or_default(),
                    to[*pair]["path"].as_str().unwrap_or_default(),
                );
            }
        }
        let mut reasons: Vec<(String, Vec<u64>)> = Vec::new();
        let (old_lines, new_lines) = (hashes(before), hashes(after));
        // A line the new text has as the old had it.
        let kept =
            |line: u64| lines.iter().any(|(&old, &new)| new as u64 == line && old_lines.get(old) == new_lines.get(new));
        let body =
            (after["body"][0].as_str().unwrap_or_default().to_string(), after["body"][1].as_u64().unwrap_or(0) as u32);
        for (index, point) in from.iter().enumerate() {
            let (Some(site), Some(pair)) = (point["site"].as_i64(), pairs[index]) else { continue };
            // The entry of the point, when the code has one.
            let number = |at: usize| to[at]["point"].as_u64().unwrap_or(0) as u32;
            // The statements to run, with those their entry needs too.
            let of_entry = |runs: &[u32]| {
                let same =
                    |list: &Vec<Value>| list.iter().map(Value::as_u64).eq(runs.iter().map(|&at| Some(at as u64)));
                to.iter().find(|other| {
                    other["entry_of"].as_u64() == Some(number(pair) as u64)
                        && other["runs"].as_array().is_some_and(same)
                })
            };
            let mut lets = passed_lets(point, &to[pair], pair, to, &paths, migrations, &kept);
            let mut entry;
            let runs = loop {
                let runs: Vec<u32> = lets.iter().map(|&at| number(at)).collect();
                entry = of_entry(&runs);
                let Some(found) = entry else { break runs };
                let more = passed_lets(point, found, pair, to, &paths, migrations, &kept);
                if more.iter().all(|at| lets.contains(at)) {
                    break runs;
                }
                lets.extend(more);
                lets.sort();
                lets.dedup();
            };
            let target = entry.unwrap_or(&to[pair]);
            let running = Entering {
                bindings: lets.iter().flat_map(|&at| list(&to[at], "binds")).filter_map(Value::as_str).collect(),
                defers: lets.iter().filter(|&&at| to[at]["kind"] == "defer").count() as u64,
                leaves: true,
            };
            let made_it = made.get(&body).and_then(|entries| entries.get(&number(pair)));
            if !runs.is_empty() && !made_it.is_some_and(|each| each.contains(&runs)) {
                wanted.entry(body.clone()).or_default().entry(number(pair)).or_default().insert(runs.clone());
            }
            // A frame in a loop of another kind starts the new one.
            let around = around_loop(&to[pair], to, &kept);
            let copied = copies(point, target, &paths, migrations, &running);
            let (target, copied) = match (copied, around) {
                (Err(why), Some(around)) if why == NEW_AROUND => {
                    (around, copies(point, around, &paths, migrations, &running))
                }
                (copied, _) => (target, copied),
            };
            match copied {
                Ok((copies, conversions)) => {
                    let line = |at: usize| after["top"].as_u64().unwrap_or(0) + to[at]["line"].as_u64().unwrap_or(0);
                    // Frames of older versions are rare: what is told is of
                    // the code that runs now.
                    if current {
                        let name = |at: usize| {
                            list(&to[at], "binds").iter().filter_map(Value::as_str).collect::<Vec<_>>().join("`, `")
                        };
                        let named = lets.iter().filter(|&&at| !list(&to[at], "binds").is_empty());
                        again.extend(named.map(|&at| format!("`{}` at line {} runs again", name(at), line(at))));
                        // What a frame keeps though its line says otherwise.
                        let written = (0..pair)
                            .filter(|&at| to[at]["changes"] == true && !kept(to[at]["line"].as_u64().unwrap_or(0)));
                        let held = written.filter(|at| !lets.contains(at));
                        again.extend(
                            held.map(|at| {
                                format!("`{}` at line {} keeps its value: it is a `var`", name(at), line(at))
                            }),
                        );
                    }
                    let word = |key: &str| target[key].as_u64().unwrap_or(0) as u32;
                    entries.push((site, slot, word("point"), word("size"), copies, conversions, passed.clone()));
                    // `passed` is what the frame's call of the new code takes.
                }
                Err(reason) => {
                    let line = after["top"].as_u64().unwrap_or(0) + target["line"].as_u64().unwrap_or(0);
                    match reasons.iter_mut().find(|(r, _)| *r == reason) {
                        Some((_, lines)) => lines.push(line),
                        None => reasons.push((reason, vec![line])),
                    }
                }
            }
        }
        let name = shown(target);
        let file = after["file"].as_str().unwrap_or_default();
        for (reason, mut lines) in reasons {
            lines.sort();
            lines.dedup();
            stay_lines.push(serde_json::json!({ "file": file, "lines": lines, "why": reason }));
            let plural = if lines.len() == 1 { "line" } else { "lines" };
            stays.push(format!(
                "a frame of `{name}` stays on the old code at {plural} {} of {file}: {reason}",
                ranges(&lines)
            ));
        }
    }
    stays.sort();
    stays.dedup();
    again.sort();
    again.dedup();
    redirects.sort();
    redirects.dedup();
    let count = entries.len();
    let mut bytes = Vec::new();
    if count > 0 || !redirects.is_empty() {
        bytes.extend(b"OSR5");
        bytes.extend((count as u32).to_le_bytes());
        for (site, slot, point, size, copies, conversions, passed) in entries {
            bytes.extend(site.to_le_bytes());
            bytes.extend([slot, point, size, copies.len() as u32].into_iter().flat_map(u32::to_le_bytes));
            for (from, to, size) in copies {
                bytes.extend([from, to, size].into_iter().flat_map(u32::to_le_bytes));
            }
            bytes.extend((conversions.len() as u32).to_le_bytes());
            for (from, to, index, name) in conversions {
                bytes.extend([from, to, index, name.len() as u32].into_iter().flat_map(u32::to_le_bytes));
                bytes.extend(name.as_bytes());
            }
            bytes.extend((passed.params.len() as u32).to_le_bytes());
            for param in &passed.params {
                let (converts, index, name) = match &param.conversion {
                    Some((index, name)) => (1, *index as u32, name.as_str()),
                    None => (0, 0, ""),
                };
                let words = [param.index, converts, u32::from(param.memory), index, param.size, name.len() as u32];
                bytes.extend(words.into_iter().flat_map(u32::to_le_bytes));
                bytes.extend(name.as_bytes());
                let (back, index, size) = param.back.map_or((0, 0, 0), |(index, size)| (1, index as u32, size));
                bytes.extend([back, index, size].into_iter().flat_map(u32::to_le_bytes));
            }
            let (returns, index, size, name) = match &passed.result {
                Some((index, size, name)) => (1, *index as u32, *size, name.as_str()),
                None => (0, 0, 0, ""),
            };
            bytes.extend([returns, index, size, name.len() as u32].into_iter().flat_map(u32::to_le_bytes));
            bytes.extend(name.as_bytes());
        }
        bytes.extend((redirects.len() as u32).to_le_bytes());
        for (from, to) in redirects {
            bytes.extend([from, to].into_iter().flat_map(u32::to_le_bytes));
        }
    }
    Moves { bytes, count, stays, stay_lines, wanted, again }
}

/// The bindings a frame at `old` passed that must run again to move to
/// `target`, the point `pair` or its entry, by their places in `to`: what the
/// new code needs there that the old does not have, and those written anew,
/// so that a change to a line above a loop that never ends takes effect. A
/// binding of the loop the frame is in comes again by itself.
fn passed_lets(
    old: &Value,
    target: &Value,
    pair: usize,
    to: &[Value],
    paths: &HashMap<&str, &str>,
    migrations: &Migrations,
    kept: &impl Fn(u64) -> bool,
) -> Vec<usize> {
    let depth = |point: &Value| point["loops"].as_u64().unwrap_or(0);
    let passed = |at: &usize| depth(&to[*at]) == 0 || depth(&to[*at]) < depth(&to[pair]);
    let binds = |at: usize, name: &str| list(&to[at], "binds").iter().any(|bound| bound == name);
    let declared = |name: &str| (0..pair).rev().filter(passed).find(|&at| binds(at, name));
    let given = given_state(old, paths);
    let line = |at: usize| to[at]["line"].as_u64().unwrap_or(0);
    // The `defer`s written since, which the old code never registered.
    let added = target["defers"].as_u64().unwrap_or(0).saturating_sub(old["defers"].as_u64().unwrap_or(0)) as usize;
    let defers: Vec<usize> =
        (0..pair).filter(passed).filter(|&at| to[at]["kind"] == "defer" && !kept(line(at))).collect();
    let mut lets = if defers.len() == added { defers } else { Vec::new() };
    for item in list(target, "state").iter().chain(list(&to[pair], "state")) {
        let key = item["key"].as_str().unwrap_or_default();
        if key.starts_with('#') {
            continue;
        }
        let Some(at) = declared(bound_name(key)) else { continue };
        // A value of a type that does not convert says why, and stays.
        let usable = given.iter().find(|(k, _)| k == key).is_some_and(|(_, from)| {
            let (before, after) =
                (from["type"].as_str().unwrap_or_default(), item["type"].as_str().unwrap_or_default());
            before == after
                || migrations.conversions.contains_key(&(before.to_string(), after.to_string()))
                || migrations.refused.contains_key(before)
        });
        // A value goes on, converted if it must, and a `var` is the state of
        // the frame; a `let` above a loop, written anew, is what the code
        // says now, unless others share what it holds.
        let same = given.iter().any(|(k, from)| k == key && from["type"] == item["type"]);
        let shared = crate::types::Handle::CONVERTED
            .iter()
            .any(|handle| item["type"].as_str().is_some_and(|ty| ty.starts_with(handle.name())));
        let written = !kept(line(at)) && to[at]["changes"] == false && depth(&to[at]) < depth(&to[pair]);
        if !usable || (written && same && !shared) {
            lets.push(at);
        }
    }
    // A `let` written anew that the loop no longer holds may have given what
    // it does hold.
    let held = |name: &&Value| {
        name.as_str().is_some_and(|name| {
            list(target, "state")
                .iter()
                .chain(list(&to[pair], "state"))
                .any(|item| item["key"].as_str().is_some_and(|key| bound_name(key) == name))
        })
    };
    lets.extend((0..pair).filter(passed).filter(|&at| {
        let bound = list(&to[at], "binds");
        !bound.is_empty()
            && !bound.iter().any(|name| held(&name))
            && !kept(line(at))
            && to[at]["changes"] == false
            && depth(&to[at]) < depth(&to[pair])
    }));
    // What reads what runs again is made of it, and runs again too, but for
    // the state of the frame.
    loop {
        let reads = |at: usize| {
            list(&to[at], "reads")
                .iter()
                .any(|read| lets.iter().any(|&ran| read.as_str().is_some_and(|name| binds(ran, name))))
        };
        let more: Vec<usize> = (0..pair)
            .filter(passed)
            .filter(|&at| {
                !lets.contains(&at) && !list(&to[at], "binds").is_empty() && to[at]["changes"] == false && reads(at)
            })
            .collect();
        if more.is_empty() {
            break;
        }
        lets.extend(more);
    }
    lets.sort();
    lets.dedup();
    lets
}

/// Why a frame does not move into a statement it was not in.
const NEW_AROUND: &str = "the statement around it is new";

/// The nearest loop around a point written anew: a frame in the old one
/// starts it.
fn around_loop<'a>(point: &Value, to: &'a [Value], kept: &impl Fn(u64) -> bool) -> Option<&'a Value> {
    let path = point["path"].as_str()?;
    let inside = |other: &&Value| {
        other["kind"] == "for"
            && !kept(other["line"].as_u64().unwrap_or(0))
            && other["path"].as_str().is_some_and(|around| path.starts_with(around) && path.len() > around.len())
    };
    to.iter().filter(inside).max_by_key(|other| other["path"].as_str().map_or(0, str::len))
}

/// What a frame takes with it to the new code: the bindings that run again,
/// which take the place of its own, the `defer`s that do, and whether what it
/// holds and the new code does not may stay unreleased.
struct Entering<'a> {
    bindings: Vec<&'a str>,
    defers: u64,
    leaves: bool,
}

fn bound_name(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(name, _)| name)
}

/// The old state by the names of the new code.
fn given_state<'a>(old: &'a Value, paths: &HashMap<&str, &str>) -> Vec<(String, &'a Value)> {
    list(old, "state")
        .iter()
        .filter_map(|item| {
            let key = item["key"].as_str()?;
            let key = match key.strip_prefix('#') {
                Some(made) => {
                    let (path, ordinal) = made.rsplit_once('/')?;
                    format!("#{}/{ordinal}", paths.get(path)?)
                }
                None => key.to_string(),
            };
            Some((key, item))
        })
        .collect()
}

/// A parameter a frame passes to a function of another identity: its
/// place, the conversion of its value and the name of its type when it
/// converts, whether it is passed in memory, and its new size there.
#[derive(Clone)]
struct Passed {
    index: u32,
    conversion: Option<(usize, String)>,
    memory: bool,
    size: u32,
    /// For a value lent to the function: the conversion that puts it back,
    /// and the old size, the caller's.
    back: Option<(usize, u32)>,
}

/// What a frame's call of a function of another identity takes: its
/// parameters, and when the result has another shape, the conversion back
/// to the one the old caller takes, the new result's size and its type.
#[derive(Clone, Default)]
struct Call {
    params: Vec<Passed>,
    result: Option<(usize, u32, String)>,
}

/// A text with the types renamed called by their new names.
pub(super) fn with_names(text: &str, renamed: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(renamed.get(word.as_str()).map_or(word.as_str(), String::as_str));
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Of the closures a closure may be now, the one whose text is most like
/// its own, if it is mostly the same and no other is as close.
fn likeliest<'a>(entry: &Value, targets: &[&'a String], new: &Value) -> Option<&'a String> {
    let text = entry["text"].as_str()?;
    let mut scored: Vec<(f64, &String)> = targets
        .iter()
        .filter_map(|&target| Some((likeness(text, new[target.as_str()]["text"].as_str()?), target)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    match scored.as_slice() {
        [(best, target), rest @ ..] if *best >= 0.5 && rest.first().is_none_or(|r| r.0 < *best) => Some(target),
        _ => None,
    }
}

/// How alike two texts are, from 0 to 1: the share of the pairs of
/// characters in a row they have in common (Dice's coefficient).
fn likeness(a: &str, b: &str) -> f64 {
    let pairs = |text: &str| {
        let chars: Vec<char> = text.split_whitespace().flat_map(|w| w.chars().chain([' '])).collect();
        let mut pairs: Vec<(char, char)> = chars.windows(2).map(|w| (w[0], w[1])).collect();
        pairs.sort_unstable();
        pairs
    };
    let (a, b) = (pairs(a), pairs(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => (i, j, common) = (i + 1, j + 1, common + 1),
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    2.0 * common as f64 / (a.len() + b.len()) as f64
}

/// How a frame of the old function passes its parameters to the new one,
/// of another identity, or why it cannot: its result must be the same, and
/// each parameter the same or convert, unless it is lent, but for the
/// captures of a closure that neither changes nor gives them away.
fn parameters(before: &Value, after: &Value, migrations: &Migrations) -> Result<Call, String> {
    let shown = |key: &str| key.split(" @").next().unwrap_or(key).to_string();
    let back = |old: &str, new: &str| migrations.backs.get(&(new.to_string(), old.to_string())).copied();
    let mut call = Call::default();
    // A result of another shape goes back to the old caller's, converted,
    // when that loses nothing.
    if before["result"] != after["result"] {
        let (old, new) = (before["result"].as_str().unwrap_or_default(), after["result"].as_str().unwrap_or_default());
        let memory = before["returns"] == "memory" && after["returns"] == "memory";
        let size = after["result_size"].as_u64().unwrap_or(0) as u32;
        match back(old, new) {
            Some(index) if memory => call.result = Some((index, size, shown(new))),
            _ => return Err("its result is of a type of another shape, which does not convert back".into()),
        }
    }
    let (from, to) = (list(before, "params"), list(after, "params"));
    if from.len() != to.len() || from.is_empty() && before.get("params").is_none() {
        return Err("its parameters or result changed".into());
    }
    let passed = &mut call.params;
    for (index, (old, new)) in from.iter().zip(to).enumerate() {
        let class = new["class"].as_str().unwrap_or_default();
        if old["class"] != new["class"] {
            return Err(format!("its parameter {} is passed another way now", index + 1));
        }
        if class == "zero" {
            continue;
        }
        let (old_type, new_type) = (old["type"].as_str().unwrap_or_default(), new["type"].as_str().unwrap_or_default());
        let memory = class == "memory";
        let size = new["size"].as_u64().unwrap_or(0) as u32;
        if old_type == new_type {
            passed.push(Passed { index: index as u32, conversion: None, memory, size, back: None });
            continue;
        }
        // A value lent goes back converted, and a closure's captures, which
        // it works on, unless it gives them away.
        let lent = old["loan"].as_bool() == Some(true);
        if lent && index == 0 && before["consumes"] == true {
            return Err("it gives its captures away, and they have another shape now".into());
        }
        let back = match back(old_type, new_type) {
            Some(index) if lent => Some((index, old["size"].as_u64().unwrap_or(0) as u32)),
            None if lent => {
                return Err(format!(
                    "its parameter {} is lent to it, of a type of another shape that does not convert back",
                    index + 1
                ));
            }
            _ => None,
        };
        match migrations.conversions.get(&(old_type.to_string(), new_type.to_string())) {
            Some(&conversion) => {
                let conversion = Some((conversion, shown(new_type)));
                passed.push(Passed { index: index as u32, conversion, memory: memory || lent, size, back });
            }
            _ => {
                return Err(migrations
                    .refused
                    .get(old_type)
                    .cloned()
                    .unwrap_or_else(|| format!("its parameter {} does not convert", index + 1)));
            }
        }
    }
    Ok(call)
}

/// A function as stacks name it.
pub(super) fn shown_name(identity: &str) -> &str {
    shown(identity)
}

/// A function as a report names it: without its parameters, result and
/// place among its function's closures.
pub(super) fn shown(identity: &str) -> &str {
    let name = identity.split('(').next().unwrap_or(identity);
    match name.rsplit_once(' ') {
        Some((closure, place)) if place.contains('/') && closure.starts_with("closure in ") => closure,
        _ => name,
    }
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

fn hashes(entry: &Value) -> Vec<u64> {
    entry["lines"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect()
}

/// Which lines of the new text each line of the old one is: those that
/// stay the same, by their longest common sequence, and those that change
/// in place, one for one where the change starts.
fn line_map(old: &[u64], new: &[u64]) -> HashMap<usize, usize> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    let width = b.len() + 1;
    let mut table = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = match a[i] == b[j] {
                true => table[(i + 1) * width + j + 1] + 1,
                false => table[(i + 1) * width + j].max(table[i * width + j + 1]),
            };
        }
    }
    let mut map: HashMap<usize, usize> = (0..prefix).map(|i| (i, i)).collect();
    for k in 0..suffix {
        map.insert(old.len() - 1 - k, new.len() - 1 - k);
    }
    let (mut i, mut j, mut hunk) = (0, 0, (0, 0));
    let changed = |map: &mut HashMap<usize, usize>, (i0, j0): (usize, usize), i: usize, j: usize| {
        for k in 0..(i - i0).min(j - j0) {
            map.insert(prefix + i0 + k, prefix + j0 + k);
        }
    };
    loop {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            changed(&mut map, hunk, i, j);
            map.insert(prefix + i, prefix + j);
            (i, j) = (i + 1, j + 1);
            hunk = (i, j);
        } else if i < a.len() && (j == b.len() || table[(i + 1) * width + j] >= table[i * width + j + 1]) {
            i += 1;
        } else if j < b.len() {
            j += 1;
        } else {
            changed(&mut map, hunk, i, j);
            return map;
        }
    }
}

/// Where each part of the state of the old point goes in that of the new,
/// as it is or through a conversion of the patch, or why the frame cannot
/// move there.
#[allow(clippy::type_complexity)]
fn copies(
    old: &Value,
    new: &Value,
    paths: &HashMap<&str, &str>,
    migrations: &Migrations,
    running: &Entering,
) -> Result<(Vec<(u32, u32, u32)>, Vec<(u32, u32, u32, String)>), String> {
    if old["defers"].as_u64().unwrap_or(0) + running.defers != new["defers"].as_u64().unwrap_or(0) {
        return Err("the `defer`s registered there changed".into());
    }
    let mut given = given_state(old, paths);
    let named = |key: &str| bound_name(key).to_string();
    // A value borrowed there keeps the frame where it is, whatever else
    // changed: the lock it is inside, or what it borrows.
    let lent = list(new, "state").iter().chain(given.iter().map(|(_, item)| *item));
    if let Some(item) = lent.into_iter().find(|item| item["lent"].as_bool() == Some(true)) {
        let key = item["key"].as_str().unwrap_or_default();
        return Err(match key.starts_with('#') {
            true => "it is inside a `lock` or borrows a value there".into(),
            false => format!("`{}` borrows a value there", named(key)),
        });
    }
    let mut copies = Vec::new();
    let mut conversions = Vec::new();
    // What runs again takes the place of what the old code has.
    let runs = |key: &str| running.bindings.contains(&bound_name(key));
    given.retain(|(key, _)| !runs(key));
    for item in list(new, "state").iter().filter(|item| !runs(item["key"].as_str().unwrap_or_default())) {
        let key = item["key"].as_str().unwrap_or_default();
        let made = key.starts_with('#');
        let Some(index) = given.iter().position(|(k, _)| k == key) else {
            return Err(match made {
                true => NEW_AROUND.into(),
                false => format!("`{}` is new there", named(key)),
            });
        };
        let (_, from) = given.swap_remove(index);
        let word = |v: &Value, key: &str| v[key].as_u64().unwrap_or(0) as u32;
        let (old_type, new_type) =
            (from["type"].as_str().unwrap_or_default(), item["type"].as_str().unwrap_or_default());
        if old_type == new_type && from["size"] == item["size"] {
            copies.push((word(from, "offset"), word(item, "offset"), word(item, "size")));
            continue;
        }
        // A value of a type of another shape converts, if it can.
        match migrations.conversions.get(&(old_type.to_string(), new_type.to_string())) {
            Some(&index) => {
                let shown = new_type.split(" @").next().unwrap_or(new_type).to_string();
                conversions.push((word(from, "offset"), word(item, "offset"), index as u32, shown));
            }
            _ => {
                return Err(match migrations.refused.get(old_type) {
                    Some(why) => why.clone(),
                    None if made => "a value of the statement around it has another type".into(),
                    None => format!("`{}` has another type", named(key)),
                });
            }
        }
    }
    // What the old code owns there and the new does not is never released:
    // a frame goes on without it, and a `goto` does not leave it behind.
    if let Some((key, _)) = given.iter().find(|(_, item)| !running.leaves && item["owned"].as_bool() == Some(true)) {
        return Err(match key.starts_with('#') {
            true => "a value the old code holds there would not be released".into(),
            false => format!("`{}` would not be released", named(key)),
        });
    }
    Ok((copies, conversions))
}

/// The patch's conversions of values of old types to new ones, by their
/// keys: each one's place in the patch's table; and why values of other
/// old types do not convert.
#[derive(Default)]
pub(super) struct Migrations {
    pub conversions: HashMap<(String, String), usize>,
    pub refused: HashMap<String, String>,
    /// The conversions back, from a new type to the old, that lose nothing.
    pub backs: HashMap<(String, String), usize>,
}

/// The move of a frame of the version `entry` of a function, at the
/// statement that starts at line `from` of its file, to the one at line
/// `to`, in the same code: as `moves` writes moves, for the debugger's
/// `goto`; or why it does not go there, as a binding the line uses that
/// holds no value yet.
pub(super) fn goto(entry: &Value, slot: u32, from: u64, to: u64) -> Result<Vec<u8>, String> {
    let top = entry["top"].as_u64().unwrap_or(0);
    let points = list(entry, "points");
    let at = |line: u64, sited: bool| {
        points.iter().find(|p| top + p["line"].as_u64().unwrap_or(0) == line && (!sited || p["site"].is_i64()))
    };
    let here =
        at(from, true).ok_or(format!("the frame is not where a statement of line {from} starts: step to one"))?;
    let there = at(to, false).ok_or(format!("no statement of this function starts at line {to}"))?;
    let paths: HashMap<&str, &str> =
        points.iter().filter_map(|p| p["path"].as_str()).map(|p| (p, p)).chain([("", "")]).collect();
    let staying = Entering { bindings: Vec::new(), defers: 0, leaves: false };
    let (copies, _) = copies(here, there, &paths, &Migrations::default(), &staying).map_err(|why| {
        let why = why.replace("is new there", &format!("holds no value at line {from} yet"));
        format!("the frame does not go to line {to}: {why}")
    })?;
    let word = |key: &str| there[key].as_u64().unwrap_or(0) as u32;
    let mut bytes = Vec::from(*b"OSR5");
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(here["site"].as_i64().unwrap_or(0).to_le_bytes());
    bytes.extend([slot, word("point"), word("size"), copies.len() as u32].into_iter().flat_map(u32::to_le_bytes));
    for (from, to, size) in copies {
        bytes.extend([from, to, size].into_iter().flat_map(u32::to_le_bytes));
    }
    // No conversion, parameter or result of another shape, nor redirect.
    bytes.extend([0u32; 7].into_iter().flat_map(u32::to_le_bytes));
    Ok(bytes)
}

/// Lines as a short list: `3, 5–8`.
fn ranges(lines: &[u64]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let start = lines[index];
        while index + 1 < lines.len() && lines[index + 1] == lines[index] + 1 {
            index += 1;
        }
        parts.push(match lines[index] == start {
            true => start.to_string(),
            false => format!("{start}–{}", lines[index]),
        });
        index += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_match_where_they_stay_and_change_in_place() {
        // a b c d → a x c y d: b becomes x, y is new.
        let map = line_map(&[1, 2, 3, 4], &[1, 9, 3, 8, 4]);
        assert_eq!((map[&0], map[&1], map[&2], map[&3]), (0, 1, 2, 4));
        // A line removed has no place.
        let map = line_map(&[1, 2, 3], &[1, 3]);
        assert_eq!((map[&0], map.get(&1), map[&2]), (0, None, 1));
        assert_eq!(ranges(&[3, 5, 6, 7, 9]), "3, 5–7, 9");
        assert!(likeness("serve((line) { \"v1 {line}\" })", "serve((line) { \"v2 {line}\" })") > 0.9);
        assert!(likeness("(line) { \"v1 {line}\" }", "() { console.println(\"bye\") }") < 0.5);
    }
}
