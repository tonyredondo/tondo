//! `tondo size` (toolchain 7.21): what an executable a build makes weighs,
//! by part (the program's code, the standard library's, its dependencies',
//! what the compiler adds, the runtime, the data, the tables that traces,
//! dumps and debuggers read, the symbols' names and the rest), and the
//! functions that weigh the most. Code a call inlines weighs in the
//! function it was copied into.

use crate::driver::{self, Analysis};
use crate::target::Target;
use object::{Object, ObjectSection, ObjectSymbol, SectionKind, SymbolKind};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

/// How a build is made, as `tondo build` takes it.
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub tests: bool,
    pub statically: bool,
    pub strip: bool,
    pub debug: bool,
    pub target: Option<Target>,
}

/// The parts of an executable, in the order they are said.
pub const PARTS: [&str; 10] = [
    "the program's code",
    "dependencies' code",
    "the standard library's code",
    "code the compiler adds",
    "the runtime's code",
    "the program's data",
    "the runtime's data",
    "tables for traces, dumps and debuggers",
    "the names of symbols",
    "the rest: headers, dynamic linking, alignment",
];

/// A function of the executable: its name, where it comes from, its part
/// and its bytes.
pub struct Function {
    pub name: String,
    pub place: String,
    pub part: &'static str,
    pub bytes: u64,
}

/// What an executable weighs.
pub struct Weighed {
    pub size: u64,
    pub parts: Vec<(&'static str, u64)>,
    pub functions: Vec<Function>,
}

/// Builds the program or the tests at `path` as `options` say, aside, and
/// weighs the executable.
pub fn weigh(path: &Path, options: Options) -> Result<Weighed, String> {
    let analysis: Analysis = match options.tests {
        true => driver::analyze_tests(path, false)?,
        false => driver::analyze(path)?,
    };
    if analysis.program.is_none() {
        let said = analysis.diags.render(&analysis.sources);
        return Err(format!("{said}{} does not compile", path.display()));
    }
    crate::codegen::name_functions(true);
    let object = driver::object(&analysis, options.debug, options.target);
    let named = crate::codegen::name_functions(false);
    let object = object?;
    let dir = std::env::temp_dir().join(format!("tondo-size-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    // Without the symbols' names, the parts are those of the build with
    // them, which name the functions, and the size the stripped one's.
    let link = |strip: bool| {
        let output = dir.join(if strip { "stripped" } else { "program" });
        let linking =
            driver::Linking { strip, statically: options.statically, target: options.target, debug: options.debug };
        driver::link(&object, &output, &analysis.libraries, linking)
            .and_then(|()| std::fs::read(&output).map_err(|e| format!("cannot read {}: {e}", output.display())))
    };
    let (linked, stripped) = (link(false), options.strip.then(|| link(true)));
    let _ = std::fs::remove_dir_all(&dir);
    let named: HashMap<String, (String, String, &'static str)> =
        named.into_iter().map(|(symbol, name, file, part)| (symbol, (name, file, part))).collect();
    let mut weighed = weigh_file(&linked?, &named)?;
    if let Some(stripped) = stripped {
        weighed.size = stripped?.len() as u64;
        weighed.parts[8].1 = 0;
        let kept: u64 = weighed.parts[..9].iter().map(|(_, bytes)| bytes).sum();
        weighed.parts[9].1 = weighed.size.saturating_sub(kept);
    }
    Ok(weighed)
}

/// The weight of an executable's bytes, its functions named by `named`.
fn weigh_file(bytes: &[u8], named: &HashMap<String, (String, String, &'static str)>) -> Result<Weighed, String> {
    let file = object::File::parse(bytes).map_err(|e| format!("the build is not an executable: {e}"))?;
    let mut parts: HashMap<&'static str, u64> = HashMap::new();
    let mut functions = Vec::new();
    // Each section's symbols, by address: a size where the format has one,
    // and up to the next symbol where it has none, as Mach-O.
    let macho = matches!(file.format(), object::BinaryFormat::MachO);
    let mut by_section: HashMap<object::SectionIndex, Vec<(u64, u64, String)>> = HashMap::new();
    for symbol in file.symbols() {
        let (Some(index), Ok(name)) = (symbol.section_index(), symbol.name()) else { continue };
        if name.is_empty() || matches!(symbol.kind(), SymbolKind::Section | SymbolKind::File) {
            continue;
        }
        let name = if macho { name.strip_prefix('_').unwrap_or(name) } else { name };
        by_section.entry(index).or_default().push((symbol.address(), symbol.size(), name.to_string()));
    }
    for section in file.sections() {
        let size = match section.file_range() {
            Some((_, size)) => size,
            None => continue,
        };
        let name = section.name().unwrap_or_default();
        let whole = match section.kind() {
            _ if name.contains("eh_frame") || name.contains("unwind_info") => Some(PARTS[7]),
            _ if name == ".symtab" || name == ".strtab" => Some(PARTS[8]),
            SectionKind::Text | SectionKind::Data | SectionKind::ReadOnlyData | SectionKind::ReadOnlyString => None,
            _ => Some(PARTS[9]),
        };
        if let Some(part) = whole {
            *parts.entry(part).or_default() += size;
            continue;
        }
        let mut symbols = by_section.remove(&section.index()).unwrap_or_default();
        symbols.sort_by_key(|(address, ..)| *address);
        symbols.dedup_by_key(|(address, ..)| *address);
        let (end, code) = (section.address() + size, section.kind() == SectionKind::Text);
        let (mut covered, mut named_bytes) = (section.address(), 0);
        for at in 0..symbols.len() {
            let (address, given, name) = &symbols[at];
            let next = symbols.get(at + 1).map_or(end, |(next, ..)| *next);
            let bytes = match macho {
                true => next.saturating_sub(*address),
                false => (*given).min(end.saturating_sub(*address)),
            };
            // Bytes another symbol counted already are not counted again.
            let from = (*address).max(covered);
            let counted = (address + bytes).saturating_sub(from);
            covered = covered.max(address + bytes);
            if counted == 0 {
                continue;
            }
            named_bytes += counted;
            let (part, shown, place) = classify(name, code, named);
            *parts.entry(part).or_default() += counted;
            if code {
                functions.push(Function { name: shown, place, part, bytes: counted });
            }
        }
        // What no symbol names is the runtime's, as constants of Rust and
        // what the C library leaves unnamed: the compiler names all it makes.
        let unnamed = size.saturating_sub(named_bytes);
        *parts.entry(if code { PARTS[4] } else { PARTS[6] }).or_default() += unnamed;
    }
    let size = bytes.len() as u64;
    let counted: u64 = parts.values().sum();
    *parts.entry(PARTS[9]).or_default() += size.saturating_sub(counted);
    functions.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    let parts = PARTS.iter().map(|part| (*part, parts.get(part).copied().unwrap_or(0))).collect();
    Ok(Weighed { size, parts, functions })
}

/// The part of a symbol, the name a person reads, and where it comes from.
fn classify(
    symbol: &str,
    code: bool,
    named: &HashMap<String, (String, String, &'static str)>,
) -> (&'static str, String, String) {
    if let Some((name, file, part)) = named.get(symbol) {
        let part = match *part {
            "program" => PARTS[0],
            "dependency" => PARTS[1],
            _ => PARTS[2],
        };
        return (part, name.clone(), file.clone());
    }
    const TABLES: [&str; 5] = ["tondo_trace", "tondo_debug", "tondo_dump", "tondo_live", "tondo_sites"];
    if symbol.starts_with("tondo_") && !symbol.starts_with("tondo_rt_") {
        let part = match (code, TABLES.iter().any(|table| symbol.starts_with(table))) {
            (_, true) => PARTS[7],
            (true, false) => PARTS[3],
            (false, false) => PARTS[5],
        };
        return (part, symbol.to_string(), "the compiler".to_string());
    }
    let shown = format!("{:#}", rustc_demangle::demangle(symbol));
    // A function of the runtime by its module, as `tondo_runtime::heap`;
    // another of Rust by its crate, as `std`; one of C alone.
    let path: Vec<&str> = shown.trim_start_matches('<').split("::").collect();
    let place = match path[..] {
        ["tondo_runtime", module, _, ..] => format!("tondo_runtime::{module}"),
        [_] => "the runtime".to_string(),
        [krate, ..] => krate.to_string(),
        [] => String::new(),
    };
    (if code { PARTS[4] } else { PARTS[6] }, shown, place)
}

/// What `weighed` says as data, its functions those whose names hold
/// `function`, `top` of them.
pub fn json(weighed: &Weighed, function: Option<&str>, top: usize) -> Value {
    let parts: Vec<Value> = weighed.parts.iter().map(|(part, bytes)| json!({ "part": part, "bytes": bytes })).collect();
    let functions: Vec<Value> = chosen(weighed, function, top)
        .map(|f| json!({ "function": f.name, "place": f.place, "part": f.part, "bytes": f.bytes }))
        .collect();
    json!({ "size": weighed.size, "parts": parts, "functions": functions })
}

fn chosen<'a>(weighed: &'a Weighed, function: Option<&'a str>, top: usize) -> impl Iterator<Item = &'a Function> {
    let asked = move |f: &&Function| function.is_none_or(|name| f.name.contains(name));
    weighed.functions.iter().filter(asked).take(top)
}

/// Bytes as a person reads them.
pub fn bytes(count: u64) -> String {
    match count {
        0..1024 => format!("{count} B"),
        1024..1_048_576 => format!("{:.1} KiB", count as f64 / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.2} MiB", count as f64 / 1_048_576.0),
        _ => format!("{:.2} GiB", count as f64 / 1_073_741_824.0),
    }
}

/// What `weighed` says for a person.
pub fn text(weighed: &Weighed, function: Option<&str>, top: usize) -> String {
    let share = |count: u64| count as f64 * 100.0 / weighed.size.max(1) as f64;
    let mut out = format!("{} in all\n", bytes(weighed.size));
    for (part, count) in weighed.parts.iter().filter(|(_, count)| *count > 0) {
        out.push_str(&format!("{:>11} {:>5.1}%  {part}\n", bytes(*count), share(*count)));
    }
    let listed: Vec<&Function> = chosen(weighed, function, top).collect();
    if !listed.is_empty() {
        out.push_str("the functions that weigh the most:\n");
        for f in listed {
            out.push_str(&format!("{:>11}  {}  ({})\n", bytes(f.bytes), f.name, f.place));
        }
    }
    out
}

/// What changed from `before`, a revision's, to `now`: each part, and each
/// function whose bytes changed, the largest change first.
pub fn changes(before: &Weighed, now: &Weighed, function: Option<&str>, top: usize) -> Value {
    let signed = |a: u64, b: u64| b as i64 - a as i64;
    let parts: Vec<Value> = now
        .parts
        .iter()
        .zip(&before.parts)
        .map(|((part, now), (_, then))| json!({ "part": part, "before": then, "bytes": now, "change": signed(*then, *now) }))
        .collect();
    // Functions of one name, as copies of a generic's glue, are counted
    // together on each side, so that one does not stand for another.
    let summed = |functions: &[Function]| {
        let mut sums: std::collections::BTreeMap<(String, String), u64> = Default::default();
        for f in functions {
            *sums.entry((f.name.clone(), f.place.clone())).or_default() += f.bytes;
        }
        sums
    };
    let (then, now_summed) = (summed(&before.functions), summed(&now.functions));
    let mut changed: Vec<(String, String, u64, u64)> = Vec::new();
    for key in then.keys().chain(now_summed.keys()).collect::<std::collections::BTreeSet<_>>() {
        let (old, new) = (then.get(key).copied().unwrap_or(0), now_summed.get(key).copied().unwrap_or(0));
        if old != new {
            changed.push((key.0.clone(), key.1.clone(), old, new));
        }
    }
    changed.retain(|(name, ..)| function.is_none_or(|asked| name.contains(asked)));
    changed.sort_by_key(|(.., old, new)| std::cmp::Reverse(signed(*old, *new).abs()));
    let functions: Vec<Value> = changed
        .iter()
        .take(top)
        .map(|(name, place, old, new)| {
            json!({ "function": name, "place": place, "before": old, "bytes": new, "change": signed(*old, *new) })
        })
        .collect();
    json!({ "before": before.size, "size": now.size, "change": signed(before.size, now.size), "parts": parts, "functions": functions })
}

/// What `changes` gives, for a person.
pub fn changes_text(changes: &Value, against: &str) -> String {
    let signed = |change: &Value| {
        let change = change.as_i64().unwrap_or(0);
        let shown = bytes(change.unsigned_abs());
        if change < 0 { format!("-{shown}") } else { format!("+{shown}") }
    };
    let mut out = format!(
        "{} now, {} at {against}: {}\n",
        bytes(changes["size"].as_u64().unwrap_or(0)),
        bytes(changes["before"].as_u64().unwrap_or(0)),
        signed(&changes["change"])
    );
    for part in changes["parts"].as_array().into_iter().flatten().filter(|p| p["change"] != 0) {
        out.push_str(&format!("{:>12}  {}\n", signed(&part["change"]), part["part"].as_str().unwrap_or_default()));
    }
    let functions = changes["functions"].as_array().cloned().unwrap_or_default();
    if !functions.is_empty() {
        out.push_str("the functions whose bytes changed the most:\n");
        for f in &functions {
            let (name, place) = (f["function"].as_str().unwrap_or_default(), f["place"].as_str().unwrap_or_default());
            out.push_str(&format!("{:>12}  {name}  ({place})\n", signed(&f["change"])));
        }
    }
    out
}
