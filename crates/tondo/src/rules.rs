//! `tondo asm --rules` (toolchain 7.19): which ISLE rules Cranelift applies
//! to each function, in a `tondo` built with the feature `rules`, which has
//! Cranelift's ISLE code log each rule that matches (`vendor/README.md`).
//! A logger counts them by the function the thread compiles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// A rule applied: the term it rewrites, where it is, as
/// `src/isa/x64/lower.isle line 207`, how many times, and whether it is
/// one of Tondo's own, marked `Tondo:`.
pub type Applied = (String, String, u64, bool);

/// How many times each rule, by its term and place, applied to a function.
type Counts = HashMap<(String, String), u64>;

static COUNTS: Mutex<Option<HashMap<String, Counts>>> = Mutex::new(None);

thread_local! {
    static COMPILING: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct Logger;

static LOGGER: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        let text = record.args().to_string();
        let Some((term, place)) = text.strip_prefix("ISLE ").and_then(|rest| rest.split_once(' ')) else { return };
        COMPILING.with(|compiling| {
            let Some(function) = compiling.borrow().clone() else { return };
            let mut counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
            let Some(counts) = counts.as_mut() else { return };
            *counts.entry(function).or_default().entry((term.to_string(), place.to_string())).or_default() += 1;
        });
    }

    fn flush(&self) {}
}

/// Counts the rules from now on, or no longer.
pub fn counting(on: bool) {
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()) = on.then(HashMap::new);
    if on {
        let _ = log::set_logger(&LOGGER);
    }
    log::set_max_level(if on { log::LevelFilter::Debug } else { log::LevelFilter::Off });
}

/// The function this thread compiles now, whose rules these are.
pub fn compiling(function: Option<&str>) {
    COMPILING.with(|compiling| *compiling.borrow_mut() = function.map(str::to_string));
}

/// The rules applied to `function`, the most first.
pub fn applied(function: &str) -> Vec<Applied> {
    let counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut applied: Vec<Applied> = counts
        .as_ref()
        .and_then(|counts| counts.get(function))
        .into_iter()
        .flatten()
        .map(|((term, place), &count)| (term.clone(), place.clone(), count, tondo(place)))
        .collect();
    applied.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    applied
}

/// The files of the code generator with rules of Tondo's own.
const OWN: [(&str, &str); 6] = [
    ("src/isa/x64/inst.isle", include_str!("../../../vendor/cranelift-codegen/src/isa/x64/inst.isle")),
    ("src/isa/x64/lower.isle", include_str!("../../../vendor/cranelift-codegen/src/isa/x64/lower.isle")),
    ("src/isa/aarch64/inst.isle", include_str!("../../../vendor/cranelift-codegen/src/isa/aarch64/inst.isle")),
    ("src/isa/aarch64/lower.isle", include_str!("../../../vendor/cranelift-codegen/src/isa/aarch64/lower.isle")),
    ("src/opts/arithmetic.isle", include_str!("../../../vendor/cranelift-codegen/src/opts/arithmetic.isle")),
    ("src/prelude_opt.isle", include_str!("../../../vendor/cranelift-codegen/src/prelude_opt.isle")),
];

/// Whether the rule at `place` is Tondo's: a comment above it, back to the
/// rule or declaration before, says `Tondo:`.
fn tondo(place: &str) -> bool {
    let Some((file, line)) = place.rsplit_once(" line ") else { return false };
    let Some(line) = line.trim().parse::<usize>().ok().filter(|&line| line > 0) else { return false };
    let Some((_, text)) = OWN.iter().find(|(name, _)| file.ends_with(name)) else { return false };
    let lines: Vec<&str> = text.lines().collect();
    for above in lines[..(line - 1).min(lines.len())].iter().rev() {
        let above = above.trim_start();
        if above.contains("Tondo:") {
            return true;
        }
        if above.starts_with("(rule") || above.starts_with("(decl") {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    /// A rule whose comment says `Tondo:` is Tondo's; the next one, past
    /// a rule of its own, is not.
    #[test]
    fn knows_tondo_s_rules() {
        let (file, text) = super::OWN[4];
        let lines: Vec<&str> = text.lines().collect();
        let marked = lines.iter().position(|line| line.contains("Tondo:")).unwrap();
        let rule = marked + lines[marked..].iter().position(|line| line.starts_with("(rule")).unwrap();
        assert!(super::tondo(&format!("{file} line {}", rule + 1)));
        let next = rule + 1 + lines[rule + 1..].iter().position(|line| line.starts_with("(rule")).unwrap();
        let between = lines[rule + 1..next].iter().any(|line| line.contains("Tondo:"));
        assert_eq!(super::tondo(&format!("{file} line {}", next + 1)), between);
        assert!(!super::tondo("src/isa/x64/lower.isle line 1"));
    }
}
