//! Which tests reach what (toolchain 7.3, 7.4): a test reaches what its code
//! names, and what that names in turn. It follows the program's names, so
//! what a value of a trait or a closure calls is reached through what names
//! it, and not through the call.

use crate::hir::TestNodeInfo;
use crate::index::{DefKind, Index};
use crate::source::{SourceMap, Span};
use std::collections::HashSet;
use std::path::Path;

/// Each name of the program: where it is, and its declaration, by place.
pub struct Names(Vec<(u32, u32, usize)>);

impl Names {
    pub fn of(index: &Index) -> Names {
        let mut names: Vec<_> = index.uses.iter().map(|&(span, def)| (span.start, span.end, def)).collect();
        names.sort_unstable();
        Names(names)
    }

    /// The declarations named inside `span`.
    fn inside(&self, span: Span) -> impl Iterator<Item = usize> + '_ {
        let from = self.0.partition_point(|name| name.0 < span.start);
        self.0[from..].iter().take_while(move |name| name.1 <= span.end).map(|name| name.2)
    }

    /// What the code in `span` reaches.
    pub fn reached(&self, index: &Index, span: Span) -> HashSet<usize> {
        let mut seen = HashSet::new();
        let mut left: Vec<usize> = self.inside(span).collect();
        while let Some(def) = left.pop() {
            if !seen.insert(def) {
                continue;
            }
            let declared = &index.defs[def];
            if !matches!(declared.kind, DefKind::Local | DefKind::Parameter | DefKind::Module) {
                left.extend(self.inside(declared.decl_span));
            }
        }
        seen
    }
}

/// The tests of a program, each with what it reaches.
pub fn reaching<'a>(index: &Index, tests: &'a [TestNodeInfo]) -> Vec<(&'a TestNodeInfo, HashSet<usize>)> {
    let names = Names::of(index);
    let tests = tests.iter().filter(|node| !node.suite && !node.id.is_empty());
    tests.map(|node| (node, names.reached(index, node.whole))).collect()
}

/// The lines of each file that changed since `rev`, as Git says them.
fn changed_lines(root: &Path, rev: &str) -> Result<Vec<(String, u32, u32)>, String> {
    let ran = std::process::Command::new("git")
        .args(["diff", "--unified=0", "--relative", rev, "--"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !ran.status.success() {
        return Err(format!("git diff {rev}: {}", String::from_utf8_lossy(&ran.stderr).trim()));
    }
    let (mut changed, mut file) = (Vec::new(), String::new());
    for line in String::from_utf8_lossy(&ran.stdout).lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            file = path.to_string();
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            let new = hunk.split_whitespace().find_map(|part| part.strip_prefix('+')).unwrap_or("0");
            let (start, count) = new.split_once(',').unwrap_or((new, "1"));
            let (start, count): (u32, u32) = (start.parse().unwrap_or(0), count.parse().unwrap_or(1));
            changed.push((file.clone(), start.max(1), start.max(1) + count.max(1) - 1));
        }
    }
    Ok(changed)
}

/// What a change of lines touches: the declarations around them, and the
/// spans of the lines of what is in no declaration.
fn touched(index: &Index, sources: &SourceMap, changed: &[(String, u32, u32)]) -> (HashSet<usize>, Vec<Span>) {
    let (mut defs, mut spans) = (HashSet::new(), Vec::new());
    for (name, first, last) in changed {
        for file in (0..sources.len()).map(|at| sources.file(at)).filter(|file| file.path.ends_with(name)) {
            let offset = |line: u32| {
                let lines = file.text.split_inclusive('\n').take(line as usize - 1);
                file.base + lines.map(str::len).sum::<usize>() as u32
            };
            let span = Span::new(offset(*first), offset(*last + 1));
            spans.push(span);
            let kept = |kind: DefKind| !matches!(kind, DefKind::Local | DefKind::Parameter | DefKind::Module);
            let overlaps = |d: Span| d.start < span.end && span.start < d.end;
            let found = index.defs.iter().enumerate().filter(|(_, d)| kept(d.kind) && overlaps(d.decl_span));
            let before = defs.len();
            defs.extend(found.map(|(at, _)| at));
            // Imports and the like: all the file may depend on them.
            if defs.len() == before {
                let whole = Span::new(file.base, file.base + file.text.len() as u32);
                let inside = |d: Span| whole.start <= d.start && d.end <= whole.end;
                let all = index.defs.iter().enumerate().filter(|(_, d)| kept(d.kind) && inside(d.decl_span));
                defs.extend(all.map(|(at, _)| at));
            }
        }
    }
    (defs, spans)
}

/// The tests of the project at `root` that reach what changed since `rev`.
pub fn tests(root: &Path, rev: &str) -> Result<Vec<String>, String> {
    let directory = if root.is_dir() { root } else { root.parent().unwrap_or(Path::new(".")) };
    let changed = changed_lines(directory, rev)?;
    let mut analysis = crate::driver::analyze_for_editor(root, root.is_dir(), Default::default())
        .map_err(|_| "the project does not compile: `tondo check` says why".to_string())?;
    let nodes = analysis.program.take().map(|program| program.tests).unwrap_or_default();
    let index = analysis.index.as_ref().ok_or("the project does not compile: `tondo check` says why")?;
    // What is not code of the program may change what any test does.
    let code = |file: &str| file.ends_with(".to");
    if changed.iter().any(|(file, ..)| !code(file) && file.ends_with("tondo.toml")) {
        return Ok(nodes.iter().filter(|n| !n.suite && !n.id.is_empty()).map(|n| n.id.clone()).collect());
    }
    let (defs, spans) = touched(index, &analysis.sources, &changed);
    let meets = |a: Span, b: Span| a.start < b.end && b.start < a.end;
    let hit = |(node, reached): &(&TestNodeInfo, HashSet<usize>)| {
        spans.iter().any(|&span| meets(span, node.whole)) || reached.iter().any(|def| defs.contains(def))
    };
    Ok(reaching(index, &nodes).iter().filter(|test| hit(test)).map(|(node, _)| node.id.clone()).collect())
}

/// What neither `main` nor a test reaches, of the project's own code: each
/// declaration's number. A library's `pub` is what others reach; the methods
/// of an `impl` are reached through their trait, and what is annotated by
/// what reads annotations.
pub fn unreached(index: &Index, sources: &SourceMap, tests: &[TestNodeInfo]) -> Vec<usize> {
    let names = Names::of(index);
    let own = |def: &crate::index::Def| sources.file_of(def.name_span.start).own;
    let before = |span: Span| {
        let file = sources.file_of(span.start);
        (&file.text[..(span.start - file.base) as usize], &file.text[(span.start - file.base) as usize..])
    };
    let program = index.defs.iter().any(|def| def.kind == DefKind::Function && def.name == "main" && own(def));
    let root = |def: &crate::index::Def| {
        let (above, from) = before(def.decl_span);
        let annotated = above.trim_end().rsplit('\n').next().is_some_and(|line| line.trim_start().starts_with('@'));
        let implemented = index.implemented.iter().any(|&(_, span)| span == def.name_span);
        (def.kind == DefKind::Function && def.name == "main")
            || implemented
            || annotated
            || (!program && from.starts_with("pub "))
    };
    let kept = |kind: DefKind| {
        matches!(kind, DefKind::Function | DefKind::Method | DefKind::Type | DefKind::Enum | DefKind::Const)
    };
    let mut reached: HashSet<usize> = HashSet::new();
    for (at, def) in index.defs.iter().enumerate().filter(|(_, def)| own(def) && root(def)) {
        reached.insert(at);
        reached.extend(names.reached(index, def.decl_span));
    }
    for test in tests {
        reached.extend(names.reached(index, test.whole));
    }
    let left = index.defs.iter().enumerate().filter(|(at, def)| kept(def.kind) && own(def) && !reached.contains(at));
    left.map(|(at, _)| at).collect()
}

/// `tondo check --unreached`: where each is.
pub fn report_unreached(root: &Path, json: bool) -> Result<(), String> {
    let mut analysis = crate::driver::analyze_for_editor(root, root.is_dir(), Default::default())
        .map_err(|_| "the project does not compile: `tondo check` says why".to_string())?;
    let nodes = analysis.program.take().map(|program| program.tests).unwrap_or_default();
    let index = analysis.index.as_ref().ok_or("the project does not compile: `tondo check` says why")?;
    let found: Vec<serde_json::Value> = unreached(index, &analysis.sources, &nodes)
        .into_iter()
        .map(|at| {
            let def = &index.defs[at];
            let (file, line, _) = analysis.sources.position(def.name_span.start);
            serde_json::json!({ "name": def.name, "file": file, "line": line })
        })
        .collect();
    if json {
        crate::cli::print_json("check-unreached", true, serde_json::json!({ "unreached": found }));
        return Ok(());
    }
    for item in &found {
        println!(
            "{}:{}: nothing reaches `{}`",
            item["file"].as_str().unwrap_or(""),
            item["line"],
            item["name"].as_str().unwrap_or("")
        );
    }
    if found.is_empty() {
        println!("main and the tests reach all the code");
    }
    Ok(())
}
