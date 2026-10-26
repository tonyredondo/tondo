//! `tondo profile` (toolchain 7.15): where a program spends its processor
//! time, what it allocates and holds, or where its tasks are, in pprof's
//! format with the function and line of every frame; and `tondo profile
//! show`, which reads a profile: the functions and lines that take the
//! most, as text or JSON, or as a flame graph in one HTML file.

use crate::observe::{self, Names, Traced};
use crate::pprof::{Builder, Frame};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a profile measures.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Cpu,
    Memory,
    Tasks,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Cpu => "cpu",
            Kind::Memory => "memory",
            Kind::Tasks => "tasks",
        }
    }
}

/// The processor time a sample stands for, in nanoseconds.
const PERIOD: i64 = 1_000_000;

/// The functions of the allocator and of its samples, which a sample of
/// memory goes through before it reaches what asked for the memory.
const ALLOCATOR: [&str; 9] = [
    "tondo_runtime::diagnostics::allocated",
    "tondo_runtime::heap::",
    "<tondo_runtime::meta::Counting",
    "__rust_alloc",
    "__rust_realloc",
    "__rdl_",
    "alloc::alloc::",
    "<alloc::raw_vec",
    "alloc::raw_vec",
];

/// Profiles a program that runs, or one it starts, until it ends,
/// `duration` passes or Ctrl-C; writes the profile and says where.
pub fn profile(
    traced: Traced,
    kind: Kind,
    duration: Option<Duration>,
    output: Option<PathBuf>,
    as_json: bool,
) -> Result<(), String> {
    let ask = match kind {
        Kind::Cpu => None,
        Kind::Memory => Some("memory"),
        Kind::Tasks => Some("dump"),
    };
    let recorded = observe::record(traced, duration, ask, kind == Kind::Memory)?;
    let mut frames = recorded.recording.frames.clone();
    if let Some(Value::Object(more)) = recorded.asked.as_ref().and_then(|a| a.get("frames")) {
        frames.extend(more.clone());
    }
    let program = recorded.info["program"].as_str().unwrap_or_default().to_string();
    let (builder, count) = build(kind, &program, &frames, &recorded.recording.samples, recorded.asked.as_ref())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let elapsed = recorded.started.elapsed();
    let bytes = builder.finish((now - elapsed).as_nanos() as i64, elapsed.as_nanos() as i64);
    let path = output.unwrap_or_else(|| {
        PathBuf::from(format!("{}-{}.{}.pprof", observe::program_name(&recorded.info), recorded.pid, kind.name()))
    });
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    if as_json {
        let fields = json!({
            "profile": path,
            "kind": kind.name(),
            "samples": count,
            "duration_ms": elapsed.as_millis() as u64,
            "lost": recorded.recording.lost,
            "ended": recorded.end,
        });
        crate::cli::print_json("profile", true, fields);
        return Ok(());
    }
    let what = match kind {
        Kind::Cpu => format!("{count} samples of 1 ms of processor time"),
        Kind::Memory => format!("{count} samples of memory"),
        Kind::Tasks => format!("{count} tasks"),
    };
    let mut said = format!("{what}: {}; `tondo profile show {}` shows where", path.display(), path.display());
    if let Some(end) = &recorded.end {
        said.push_str(&format!("; {end}"));
    }
    eprintln!("tondo: {said}");
    Ok(())
}

/// The profile of `kind` of what a program said: the samples of its
/// threads, or the answer it gave to what it was asked, `memory` or
/// `dump`, with the frames they name; and how many samples it has.
pub(crate) fn build(
    kind: Kind,
    program: &str,
    frames: &serde_json::Map<String, Value>,
    samples: &[Value],
    asked: Option<&Value>,
) -> Result<(Builder, usize), String> {
    let names = Names::new(frames, program);
    let stack = |addresses: &Value| -> Vec<(u64, String, String, i64)> {
        addresses
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|address| {
                let frame = &frames[address];
                let file = frame["file"].as_str().unwrap_or_default().to_string();
                let line = frame["line"].as_i64().unwrap_or(0);
                (u64::from_str_radix(address, 16).unwrap_or(0), names.function(address), file, line)
            })
            .collect()
    };
    let add = |builder: &mut Builder, addresses: &Value, values: &[i64], labels: &[(&str, &str)]| {
        let mut frames = stack(addresses);
        // The allocator and its sampling are not where the memory was
        // asked for.
        if kind == Kind::Memory {
            let machinery = |name: &str| ALLOCATOR.iter().any(|prefix| name.starts_with(prefix));
            let asked = frames.iter().position(|(_, function, ..)| !machinery(function)).unwrap_or(0);
            frames.drain(..asked);
        }
        let frames: Vec<Frame> = frames
            .iter()
            .map(|(address, function, file, line)| Frame { address: *address, function, file, line: *line })
            .collect();
        builder.sample(&frames, values, labels);
    };
    match kind {
        Kind::Cpu => {
            let mut builder =
                Builder::new(program, &[("samples", "count"), ("cpu", "nanoseconds")], ("cpu", "nanoseconds"), PERIOD);
            for sample in samples {
                let thread = sample[1].to_string();
                add(&mut builder, &sample[2], &[1, PERIOD], &[("thread", &thread)]);
            }
            Ok((builder, samples.len()))
        }
        Kind::Memory => {
            let memory = asked.map(|a| a["memory"].clone()).filter(|m| m.is_object()).ok_or(
                "the program said nothing of its memory: it ended before it was asked, or it does not sample it",
            )?;
            let rate = memory["rate"].as_f64().unwrap_or(512.0 * 1024.0);
            let types = [
                ("alloc_objects", "count"),
                ("alloc_space", "bytes"),
                ("inuse_objects", "count"),
                ("inuse_space", "bytes"),
            ];
            let mut builder = Builder::new(program, &types, ("space", "bytes"), rate as i64);
            builder.default_type("inuse_space");
            let samples = memory["samples"].as_array().cloned().unwrap_or_default();
            for sample in &samples {
                let size = sample["size"].as_f64().unwrap_or(0.0);
                // A sample stands for 1 / (1 − e^(−size / rate)) allocations.
                let weight = 1.0 / (1.0 - (-size / rate).exp());
                let values = [0, 0, weight.round() as i64, (weight * size).round() as i64];
                add(&mut builder, &sample["stack"], &values, &[]);
            }
            let allocated = memory["allocated"].as_array().cloned().unwrap_or_default();
            for entry in &allocated {
                let (objects, bytes) =
                    (entry["objects"].as_f64().unwrap_or(0.0), entry["bytes"].as_f64().unwrap_or(0.0));
                add(&mut builder, &entry["stack"], &[objects.round() as i64, bytes.round() as i64, 0, 0], &[]);
            }
            Ok((builder, samples.len() + allocated.len()))
        }
        Kind::Tasks => {
            let dump = asked.filter(|d| d["tasks"].is_array()).ok_or(
                "the program ended before its tasks could be seen: give --duration to profile them while it runs",
            )?;
            let mut builder = Builder::new(program, &[("tasks", "count")], ("tasks", "count"), 1);
            let tasks = dump["tasks"].as_array().cloned().unwrap_or_default();
            for task in &tasks {
                let state = task["state"].as_str().unwrap_or_default().to_string();
                add(&mut builder, &task["stack"], &[1], &[("state", &state)]);
            }
            Ok((builder, tasks.len()))
        }
    }
}

// ---------------------------------------------------------------------------
// Reading profiles
// ---------------------------------------------------------------------------

/// A frame as `show` reads it: its function, file and line.
pub type Located = (String, String, i64);

/// A profile as `show` reads it.
pub struct Profile {
    pub sample_types: Vec<(String, String)>,
    pub default_type: String,
    /// Each sample's frames, innermost first, as function, file and line,
    /// and its values.
    pub samples: Vec<(Vec<Located>, Vec<i64>)>,
    pub duration: i64,
}

/// Reads a profile in pprof's format, gzipped or not.
pub fn read(path: &Path) -> Result<Profile, String> {
    let raw = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let bytes = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&raw[..])
            .read_to_end(&mut out)
            .map_err(|e| format!("{} is not a gzipped profile: {e}", path.display()))?;
        out
    } else {
        raw
    };
    decode(&bytes).ok_or(format!("{} is not a profile in pprof's format", path.display()))
}

/// The fields of a protobuf message: each number with its integer, or its
/// bytes when it is length-delimited.
fn fields(mut bytes: &[u8]) -> Option<Vec<(u64, u64, &[u8])>> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let key = varint(&mut bytes)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => out.push((field, varint(&mut bytes)?, &[][..])),
            1 => {
                let (word, rest) = bytes.split_at_checked(8)?;
                out.push((field, u64::from_le_bytes(word.try_into().ok()?), &[][..]));
                bytes = rest;
            }
            2 => {
                let length = varint(&mut bytes)? as usize;
                let (data, rest) = bytes.split_at_checked(length)?;
                out.push((field, 0, data));
                bytes = rest;
            }
            5 => bytes = bytes.get(4..)?,
            _ => return None,
        }
    }
    Some(out)
}

fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let (mut value, mut shift) = (0u64, 0);
    loop {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
}

/// The integers of a repeated field, packed or not.
fn integers(value: u64, data: &[u8]) -> Vec<u64> {
    if data.is_empty() {
        return vec![value];
    }
    let mut bytes = data;
    let mut out = Vec::new();
    while let Some(value) = varint(&mut bytes) {
        out.push(value);
    }
    out
}

fn decode(bytes: &[u8]) -> Option<Profile> {
    let mut strings: Vec<String> = Vec::new();
    let mut types = Vec::new();
    let mut samples = Vec::new();
    let mut locations: HashMap<u64, Vec<(u64, i64)>> = HashMap::new();
    let mut functions: HashMap<u64, (u64, u64)> = HashMap::new();
    let (mut duration, mut default_type) = (0, 0);
    for (field, value, data) in fields(bytes)? {
        match field {
            1 => {
                let parts = fields(data)?;
                let get = |n| parts.iter().find(|(f, ..)| *f == n).map_or(0, |(_, v, _)| *v);
                types.push((get(1), get(2)));
            }
            2 => {
                let (mut ids, mut values) = (Vec::new(), Vec::new());
                for (field, value, data) in fields(data)? {
                    match field {
                        1 => ids.extend(integers(value, data)),
                        2 => values.extend(integers(value, data).into_iter().map(|v| v as i64)),
                        _ => {}
                    }
                }
                samples.push((ids, values));
            }
            4 => {
                let (mut id, mut lines) = (0, Vec::new());
                for (field, value, data) in fields(data)? {
                    match field {
                        1 => id = value,
                        4 => {
                            let parts = fields(data)?;
                            let get = |n| parts.iter().find(|(f, ..)| *f == n).map_or(0, |(_, v, _)| *v);
                            lines.push((get(1), get(2) as i64));
                        }
                        _ => {}
                    }
                }
                locations.insert(id, lines);
            }
            5 => {
                let parts = fields(data)?;
                let get = |n| parts.iter().find(|(f, ..)| *f == n).map_or(0, |(_, v, _)| *v);
                functions.insert(get(1), (get(2), get(4)));
            }
            6 => strings.push(String::from_utf8_lossy(data).into_owned()),
            10 => duration = value as i64,
            14 => default_type = value,
            _ => {}
        }
    }
    let text = |index: u64| strings.get(index as usize).cloned().unwrap_or_default();
    let sample_types: Vec<(String, String)> = types.iter().map(|&(t, u)| (text(t), text(u))).collect();
    let samples = samples
        .into_iter()
        .map(|(ids, values)| {
            // A location may hold the lines of functions inlined into
            // another, the innermost first.
            let frames = ids
                .iter()
                .flat_map(|id| locations.get(id).cloned().unwrap_or_default())
                .map(|(function, line)| {
                    let (name, file) = functions.get(&function).copied().unwrap_or((0, 0));
                    (text(name), text(file), line)
                })
                .collect();
            (frames, values)
        })
        .collect();
    let default_type = match default_type {
        0 => sample_types.last().map(|(t, _)| t.clone()).unwrap_or_default(),
        index => text(index),
    };
    Some(Profile { sample_types, default_type, samples, duration })
}

/// What takes the most in a profile, by one of its sample types: the
/// total, each function's own (`flat`) and with what it calls (`cum`),
/// and each line's: its own being what the innermost line of the
/// program's code in a stack takes, with the runtime and the standard
/// library it calls, so that it points at the program.
pub struct Hot {
    pub kind: String,
    pub unit: String,
    pub total: i64,
    pub functions: Vec<(String, String, i64, i64)>,
    pub lines: Vec<(String, i64, String, i64, i64)>,
    /// The stacks, outermost first, with their values, for a flame graph.
    pub stacks: Vec<(Vec<String>, i64)>,
}

pub fn hot(profile: &Profile, kind: Option<&str>) -> Result<Hot, String> {
    let wanted = kind.unwrap_or(&profile.default_type);
    let index = profile.sample_types.iter().position(|(t, _)| t == wanted).ok_or_else(|| {
        let known: Vec<&str> = profile.sample_types.iter().map(|(t, _)| t.as_str()).collect();
        format!("the profile has no `{wanted}`: it has {}", known.join(", "))
    })?;
    let unit = profile.sample_types[index].1.clone();
    let mut total = 0;
    let mut functions: HashMap<(String, String), (i64, i64)> = HashMap::new();
    let mut lines: HashMap<(String, i64), (String, i64, i64)> = HashMap::new();
    let mut stacks: HashMap<Vec<String>, i64> = HashMap::new();
    for (frames, values) in &profile.samples {
        let value = values.get(index).copied().unwrap_or(0);
        if value == 0 || frames.is_empty() {
            continue;
        }
        total += value;
        let mut seen_functions = std::collections::HashSet::new();
        let mut seen_lines = std::collections::HashSet::new();
        let located = |(_, file, line): &&(String, String, i64)| *line > 0 && !file.is_empty();
        let own = frames
            .iter()
            .position(|frame| located(&frame) && !frame.1.starts_with("<std>"))
            .or_else(|| frames.iter().position(|frame| located(&frame)));
        for (depth, (function, file, line)) in frames.iter().enumerate() {
            let flat = if depth == 0 { value } else { 0 };
            let key = (function.clone(), file.clone());
            if seen_functions.insert(key.clone()) {
                let entry = functions.entry(key).or_default();
                entry.0 += flat;
                entry.1 += value;
            } else if depth == 0 {
                functions.entry(key).or_default().0 += flat;
            }
            if *line > 0 && !file.is_empty() && seen_lines.insert((file.clone(), *line)) {
                let entry = lines.entry((file.clone(), *line)).or_insert((function.clone(), 0, 0));
                entry.1 += if own == Some(depth) { value } else { 0 };
                entry.2 += value;
            }
        }
        let names: Vec<String> = frames.iter().rev().map(|(function, ..)| function.clone()).collect();
        *stacks.entry(names).or_default() += value;
    }
    let mut functions: Vec<(String, String, i64, i64)> =
        functions.into_iter().map(|((function, file), (flat, cum))| (function, file, flat, cum)).collect();
    functions.sort_by(|a, b| b.2.cmp(&a.2).then(b.3.cmp(&a.3)).then(a.0.cmp(&b.0)));
    let mut lines: Vec<(String, i64, String, i64, i64)> =
        lines.into_iter().map(|((file, line), (function, flat, cum))| (file, line, function, flat, cum)).collect();
    lines.sort_by(|a, b| b.3.cmp(&a.3).then(b.4.cmp(&a.4)).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    let mut stacks: Vec<(Vec<String>, i64)> = stacks.into_iter().collect();
    stacks.sort();
    Ok(Hot { kind: wanted.to_string(), unit, total, functions, lines, stacks })
}

/// A value in its unit, as people read it.
pub fn amount(value: i64, unit: &str) -> String {
    let value = value as f64;
    match unit {
        "nanoseconds" => match value {
            v if v >= 1e9 => format!("{:.2}s", v / 1e9),
            v if v >= 1e6 => format!("{:.0}ms", v / 1e6),
            v if v >= 1e3 => format!("{:.0}us", v / 1e3),
            v => format!("{v:.0}ns"),
        },
        "bytes" => match value {
            v if v >= 1024.0 * 1024.0 * 1024.0 => format!("{:.1}GiB", v / (1024.0 * 1024.0 * 1024.0)),
            v if v >= 1024.0 * 1024.0 => format!("{:.1}MiB", v / (1024.0 * 1024.0)),
            v if v >= 1024.0 => format!("{:.1}KiB", v / 1024.0),
            v => format!("{v:.0}B"),
        },
        _ => format!("{value:.0}"),
    }
}

/// The source of a line, when its file is here to read.
fn source(file: &str, line: i64) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().nth(line as usize - 1).map(|l| l.trim().to_string())
}

/// `tondo profile show`: the functions, or with `lines` the lines, that
/// take the most, `top` of them; as JSON; or a flame graph in `html`.
pub fn show(
    path: &Path,
    kind: Option<&str>,
    top: usize,
    lines: bool,
    html: Option<&Path>,
    as_json: bool,
) -> Result<(), String> {
    let profile = read(path)?;
    let hot = hot(&profile, kind)?;
    let percent = |value: i64| if hot.total == 0 { 0.0 } else { value as f64 * 100.0 / hot.total as f64 };
    if let Some(html) = html {
        let page = flame_page(&path.display().to_string(), &hot);
        std::fs::write(html, page).map_err(|e| format!("cannot write {}: {e}", html.display()))?;
    }
    if as_json {
        let functions: Vec<Value> = hot
            .functions
            .iter()
            .take(top)
            .map(|(function, file, flat, cum)| json!({ "function": function, "file": file, "flat": flat, "cum": cum }))
            .collect();
        let lines: Vec<Value> = hot.lines.iter().take(top).map(|(file, line, function, flat, cum)| {
            json!({ "file": file, "line": line, "function": function, "flat": flat, "cum": cum, "source": source(file, *line) })
        }).collect();
        let fields = json!({
            "profile": path,
            "type": hot.kind,
            "unit": hot.unit,
            "types": profile.sample_types.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
            "total": hot.total,
            "duration_ms": profile.duration / 1_000_000,
            "functions": functions,
            "lines": lines,
            "html": html,
        });
        crate::cli::print_json("profile-show", true, fields);
        return Ok(());
    }
    let over = match profile.duration {
        0 => String::new(),
        nanos => format!(" over {:.2}s", nanos as f64 / 1e9),
    };
    let mut out = format!("{} of {}, {} in all{over}\n", hot.kind, path.display(), amount(hot.total, &hot.unit));
    if lines {
        out.push_str(&format!("{:>9} {:>6} {:>9} {:>6}  line\n", "flat", "flat%", "cum", "cum%"));
        for (file, line, function, flat, cum) in hot.lines.iter().take(top) {
            let text = source(file, *line).map_or(String::new(), |s| format!("  {s}"));
            out.push_str(&format!(
                "{:>9} {:>5.1}% {:>9} {:>5.1}%  {file}:{line} in {function}{text}\n",
                amount(*flat, &hot.unit),
                percent(*flat),
                amount(*cum, &hot.unit),
                percent(*cum),
            ));
        }
    } else {
        out.push_str(&format!("{:>9} {:>6} {:>9} {:>6}  function\n", "flat", "flat%", "cum", "cum%"));
        for (function, file, flat, cum) in hot.functions.iter().take(top) {
            let file = if file.is_empty() { String::new() } else { format!("  {file}") };
            out.push_str(&format!(
                "{:>9} {:>5.1}% {:>9} {:>5.1}%  {function}{file}\n",
                amount(*flat, &hot.unit),
                percent(*flat),
                amount(*cum, &hot.unit),
                percent(*cum),
            ));
        }
    }
    if let Some(html) = html {
        out.push_str(&format!("the flame graph is in {}\n", html.display()));
    }
    print!("{out}");
    Ok(())
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A node of the tree of calls: its value and its children, by name.
#[derive(Default)]
struct Node {
    value: i64,
    children: Vec<(String, Node)>,
}

/// The boxes of a flame graph as they are drawn: how wide a unit is, the
/// total, the unit, how deep it got, and the boxes so far, each level's
/// height to fill in once the depth is known.
struct Drawing<'a> {
    scale: f64,
    total: i64,
    unit: &'a str,
    depth: usize,
    out: String,
}

impl Drawing<'_> {
    fn draw(&mut self, node: &Node, x: f64, level: usize) {
        let mut at = x;
        for (name, child) in &node.children {
            let w = child.value as f64 * self.scale;
            if w >= 0.5 {
                self.depth = self.depth.max(level + 1);
                let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 40;
                let share = child.value as f64 * 100.0 / self.total.max(1) as f64;
                let label = match w > 40.0 {
                    true => escape(&name.chars().take((w / 7.0) as usize).collect::<String>()),
                    false => String::new(),
                };
                self.out.push_str(&format!(
                    "<g><title>{} — {} ({share:.1}%)</title><rect x=\"{at:.1}\" y=\"{{Y{level}}}\" width=\"{w:.1}\" height=\"17\" fill=\"hsl({},80%,60%)\"/><text x=\"{:.1}\" y=\"{{T{level}}}\">{label}</text></g>\n",
                    escape(name),
                    amount(child.value, self.unit),
                    10 + hue,
                    at + 3.0
                ));
                self.draw(child, at, level + 1);
            }
            at += w;
        }
    }
}

/// A flame graph of the stacks in one HTML page without scripts: each
/// function a box as wide as its share, above what called it; its title
/// says how much.
pub fn flame_page(title: &str, hot: &Hot) -> String {
    let mut root = Node::default();
    for (stack, value) in &hot.stacks {
        root.value += value;
        let mut node = &mut root;
        for name in stack {
            let at = match node.children.iter().position(|(n, _)| n == name) {
                Some(at) => at,
                None => {
                    node.children.push((name.clone(), Node::default()));
                    node.children.len() - 1
                }
            };
            node = &mut node.children[at].1;
            node.value += value;
        }
    }
    let (width, row) = (1200.0, 18.0);
    let scale = if root.value == 0 { 0.0 } else { width / root.value as f64 };
    let mut drawing = Drawing { scale, total: root.value, unit: &hot.unit, depth: 0, out: String::new() };
    drawing.draw(&root, 0.0, 0);
    let (depth, mut boxes) = (drawing.depth, drawing.out);
    // Callers at the bottom, as flame graphs are read.
    let height = depth as f64 * row + 4.0;
    for level in 0..depth {
        let y = height - (level + 1) as f64 * row;
        boxes = boxes
            .replace(&format!("{{Y{level}}}"), &format!("{y:.1}"))
            .replace(&format!("{{T{level}}}"), &format!("{:.1}", y + 13.0));
    }
    let percent = |value: i64| if hot.total == 0 { 0.0 } else { value as f64 * 100.0 / hot.total as f64 };
    let mut rows = String::new();
    for (function, file, flat, cum) in hot.functions.iter().take(30) {
        rows.push_str(&format!(
            "<tr><td class=\"n\">{}</td><td class=\"n\">{:.1}%</td><td class=\"n\">{}</td><td class=\"n\">{:.1}%</td><td><code>{}</code> {}</td></tr>\n",
            amount(*flat, &hot.unit), percent(*flat), amount(*cum, &hot.unit), percent(*cum), escape(function), escape(file)
        ));
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title><style>\
body{{font:14px/1.5 system-ui,sans-serif;margin:2rem auto;max-width:76rem;padding:0 1rem;color:#1f2328}}\
svg text{{font:11px monospace;pointer-events:none}}table{{border-collapse:collapse}}\
td,th{{padding:.2rem .6rem;border-bottom:1px solid #d0d7de;text-align:left}}td.n{{text-align:right;font-variant-numeric:tabular-nums}}\
</style></head><body>\n<h1>{title}</h1>\n<p>{} of {} in all. Each box is a function, as wide as its share, above what called it; hover it for how much.</p>\n\
<svg width=\"{width}\" height=\"{height:.0}\" role=\"img\">\n{boxes}</svg>\n<h2>Functions</h2>\n<table><tr><th>flat</th><th>flat%</th><th>cum</th><th>cum%</th><th>function</th></tr>\n{rows}</table>\n</body></html>\n",
        escape(&hot.kind),
        amount(hot.total, &hot.unit),
        title = escape(title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let mut builder =
            Builder::new("app", &[("samples", "count"), ("cpu", "nanoseconds")], ("cpu", "nanoseconds"), PERIOD);
        let frame = |address, function, line| Frame { address, function, file: "src/main.to", line };
        builder.sample(&[frame(0x10, "parse", 4), frame(0x20, "main", 9)], &[1, PERIOD], &[("thread", "1")]);
        builder.sample(&[frame(0x10, "parse", 4), frame(0x20, "main", 9)], &[1, PERIOD], &[]);
        builder.sample(&[frame(0x30, "main", 10)], &[1, PERIOD], &[]);
        let path = std::env::temp_dir().join(format!("tondo-profile-test-{}.pprof", std::process::id()));
        std::fs::write(&path, builder.finish(0, 3 * PERIOD)).unwrap();
        let read = read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(read.default_type, "cpu");
        let hot = hot(&read, None).unwrap();
        assert_eq!(hot.total, 3 * PERIOD);
        let functions: Vec<(&str, i64, i64)> =
            hot.functions.iter().map(|(f, _, flat, cum)| (f.as_str(), *flat, *cum)).collect();
        assert_eq!(functions, [("parse", 2 * PERIOD, 2 * PERIOD), ("main", PERIOD, 3 * PERIOD)]);
        let lines: Vec<(i64, i64, i64)> =
            hot.lines.iter().map(|(_, line, _, flat, cum)| (*line, *flat, *cum)).collect();
        assert_eq!(lines, [(4, 2 * PERIOD, 2 * PERIOD), (10, PERIOD, PERIOD), (9, 0, 2 * PERIOD)]);
        assert_eq!(amount(2 * PERIOD, "nanoseconds"), "2ms");
        let page = flame_page("cpu", &hot);
        assert!(page.contains("<title>parse — 2ms (66.7%)</title>") && !page.contains("<script"), "{page}");
    }
}
