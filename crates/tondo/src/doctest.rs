//! `tondo doc-test` (21.6): checks the examples of a Markdown document,
//! each by its header: syntax, a fragment or a script that compiles
//! against a fixture of Appendix C, an example that fails with the errors
//! it names, or pseudocode, and gives a record of each as JSON. It never
//! runs an example.

use crate::diag::Diagnostics;
use crate::source::SourceMap;
use serde_json::{Value, json};
use std::path::Path;

/// The fixtures of Appendix C, in their canonical serialization (C.6).
const MANIFEST: &str = include_str!("../../../conformance/0.1/fixtures/tondo-fixture-manifest.txt");

/// The modules of the fixtures' universe that the toolchain provides
/// itself: what the manifest declares of them is a part of what they have,
/// and a pipeline runs only through the real `std.process`.
const OWN: [&str; 2] = ["console", "process"];

/// The codes of errors in the registry (22.2).
pub const REGISTRY: &str = "E0001 E0002 E0003 E0004 E0005 E0006 E1001 E1002 E1003 E1004 E1005 E1006 E1007 \
E1008 E1101 E1102 E1103 E1104 E1105 E1106 E1107 E1108 E1109 E1110 E1111 E1112 E1113 E1114 E1115 E1116 E1117 \
E1201 E1202 E1203 E1204 E1205 E1206 E1301 E1302 E1303 E1304 E1401 E1402 E1403 E1405 E1406 E1407 E1410 E1411 \
E1501 E1502 E1503 E1504 E1505 E1605 E1609 E1611 E1612 E1613 E1615 E1701 E1702 E1703 E1704 E1705 E1706 E1801 E1802 \
E1803 E1804 E1805 E1806 E1807 E1901 E1902 E1903 E2001 E2002 E2003 E2004 E2005 E2101 E2102 E2103 E2104 E2105 E2106 \
E2107 E2108 E2109 E2110";

#[derive(Copy, Clone, PartialEq)]
enum Category {
    Syntax,
    Fragment,
    Script,
    CompileFail,
    Pseudocode,
}

struct Fence {
    /// Where its opening starts, and its line.
    byte: usize,
    line: usize,
    category: Category,
    fixture: Option<String>,
    codes: Vec<String>,
    source: String,
}

/// Why the document itself fails: the code of 22.2, its line and why.
pub struct DocumentError {
    pub code: &'static str,
    pub line: usize,
    pub message: String,
}

/// The record of each example, and what each one that fails reports.
pub struct Report {
    pub records: Value,
    pub failures: Vec<String>,
}

/// Checks the examples of the document `file`.
pub fn check(file: &str, bytes: &[u8]) -> Result<Report, DocumentError> {
    let fences = fences(bytes)?;
    let manifest = Manifest::parse(MANIFEST);
    let manifest_hash = crate::hash::sha256_hex(MANIFEST.as_bytes());
    let mut records = Vec::new();
    let mut failures = Vec::new();
    for fence in &fences {
        let fixture = match fence.category {
            Category::Syntax | Category::Pseudocode => None,
            _ => Some(fence.fixture.clone().unwrap_or_else(|| "spec.0_1".into())),
        };
        if let Some(name) = &fixture
            && manifest.chain(name).is_none()
        {
            let message = format!("there is no fixture `{name}`: Appendix C defines them");
            return Err(DocumentError { code: "E0006", line: fence.line, message });
        }
        let (record, failure) = example(file, fence, fixture.as_deref(), &manifest);
        let mut record = record;
        record["fixture"] = json!(fixture);
        record["fixture_sha256"] = json!(fixture.as_ref().map(|_| &manifest_hash));
        records.push(record);
        failures.extend(failure.map(|why| format!("{file}:{}: {why}", fence.line)));
    }
    Ok(Report { records: Value::Array(records), failures })
}

/// Checks one example: its record, and why it fails if it does.
fn example(file: &str, fence: &Fence, fixture: Option<&str>, manifest: &Manifest) -> (Value, Option<String>) {
    let category = match fence.category {
        Category::Syntax => "syntax",
        Category::Fragment => "fragment",
        Category::Script => "script",
        Category::CompileFail => "compile-fail",
        Category::Pseudocode => "pseudocode",
    };
    let mut record = json!({
        "file": file,
        "fence_byte": fence.byte,
        "category": category,
        "edition": "0.1",
        "production": null,
        "source_sha256": crate::hash::sha256_hex(fence.source.as_bytes()),
        "formatted_sha256": null,
        "parse_ok": null,
        "typecheck_ok": null,
        "expected_codes": fence.codes,
        "actual_codes": [],
    });
    if fence.category == Category::Pseudocode {
        return (record, None);
    }
    let name = format!("{file}#{}", fence.line);
    let mut sources = SourceMap::default();
    let id = sources.add(Path::new(&name), fence.source.clone());
    let mut parsed = Diagnostics::default();
    crate::parser::parse_file(sources.file(id), &mut parsed);
    let formatted = || crate::fmt::format(sources.file(id), &mut Diagnostics::default());
    if fence.category == Category::Syntax {
        let productions = crate::parser::syntax_productions(sources.file(id));
        record["parse_ok"] = json!(productions.is_some());
        let Some(productions) = productions else {
            record["actual_codes"] = json!(codes(&parsed));
            let why = format!("this example is not Tondo syntax:\n{}", parsed.render(&sources));
            return (record, Some(why));
        };
        record["production"] = json!(productions);
        record["formatted_sha256"] = json!(formatted().map(|text| crate::hash::sha256_hex(text.as_bytes())));
        return (record, None);
    }
    let parse_ok = !parsed.has_errors();
    record["parse_ok"] = json!(parse_ok);
    if parse_ok {
        let production: &[&str] = match fence.category {
            Category::Script => &["script_program"],
            _ => &["script_program", "fragment_wrapper"],
        };
        record["production"] = json!(production);
        record["formatted_sha256"] = json!(formatted().map(|text| crate::hash::sha256_hex(text.as_bytes())));
    }
    let (tail, universe) = manifest.generate(fixture.unwrap_or("spec.0_1"));
    // A script compiles against the fixture's modules alone.
    let tail = if fence.category == Category::Script { String::new() } else { tail };
    let (sources, diags) = crate::driver::analyze_fence(&name, &fence.source, &tail, &universe);
    let actual = codes(&diags);
    let errors: Vec<&String> = actual.iter().filter(|c| c.starts_with('E')).collect();
    if parse_ok {
        record["typecheck_ok"] = json!(errors.is_empty());
    }
    record["actual_codes"] = json!(actual);
    let why = match fence.category {
        Category::CompileFail if errors.iter().copied().eq(fence.codes.iter()) => None,
        Category::CompileFail if errors.is_empty() => {
            Some(format!("this example should fail with {}, and it compiles", fence.codes.join(", ")))
        }
        Category::CompileFail => Some(format!(
            "this example should fail with {}, not with:\n{}",
            fence.codes.join(", "),
            diags.render(&sources)
        )),
        _ if errors.is_empty() => None,
        _ => Some(format!("this example does not compile:\n{}", diags.render(&sources))),
    };
    (record, why)
}

/// The distinct codes of the diagnostics, in order.
fn codes(diags: &Diagnostics) -> Vec<String> {
    let mut codes: Vec<String> = diags.list.iter().filter_map(|d| d.code).map(str::to_string).collect();
    codes.sort();
    codes.dedup();
    codes
}

/// The examples of a document: each `~~~tondo` line at the start of a line
/// opens one, and the next line that is exactly `~~~` closes it.
fn fences(bytes: &[u8]) -> Result<Vec<Fence>, DocumentError> {
    let text = std::str::from_utf8(bytes).map_err(|e| {
        let line = bytes[..e.valid_up_to()].iter().filter(|&&b| b == b'\n').count() + 1;
        DocumentError { code: "E0001", line, message: "the document is not valid UTF-8".into() }
    })?;
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let line = match raw.strip_suffix('\n') {
            Some(line) => line.strip_suffix('\r').unwrap_or(line),
            None => raw,
        };
        if line.contains('\r') {
            let message = "a carriage return ends a line only before a line feed".into();
            return Err(DocumentError { code: "E0002", line: lines.len() + 1, message });
        }
        lines.push((offset, line));
        offset += raw.len();
    }
    let mut fences = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (byte, line) = lines[index];
        let Some(header) = line.strip_prefix("~~~tondo") else {
            index += 1;
            continue;
        };
        let error = |message: String| DocumentError { code: "E0006", line: index + 1, message };
        let Some(length) = lines[index + 1..].iter().position(|(_, l)| *l == "~~~") else {
            return Err(error("this example has no closing `~~~` line".into()));
        };
        let body = &lines[index + 1..index + 1 + length];
        let mut source: String = body.iter().map(|(_, l)| *l).collect::<Vec<_>>().join("\n");
        source.push('\n');
        let (category, fixture, codes) = parse_header(header).map_err(error)?;
        fences.push(Fence { byte, line: index + 1, category, fixture, codes, source });
        index += length + 2;
    }
    Ok(fences)
}

/// What a header after `~~~tondo` says: the category, the fixture and the
/// codes of a `compile-fail`.
fn parse_header(header: &str) -> Result<(Category, Option<String>, Vec<String>), String> {
    let unknown = || {
        format!(
            "unknown header `tondo{header}`: write `tondo`, `tondo fragment [fixture]`, `tondo script [fixture]`, `tondo compile-fail E0000 ...` or `tondo pseudocode`"
        )
    };
    if header.is_empty() {
        return Ok((Category::Syntax, None, Vec::new()));
    }
    let words: Vec<&str> = header.strip_prefix(' ').ok_or_else(unknown)?.split(' ').collect();
    if words.iter().any(|w| w.is_empty()) {
        return Err(unknown());
    }
    let fixture = |name: &str| {
        let mut chars = name.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
        if valid { Ok(Some(name.to_string())) } else { Err(format!("`{name}` is not a fixture name")) }
    };
    match words.as_slice() {
        ["fragment"] => Ok((Category::Fragment, None, Vec::new())),
        ["fragment", name] => Ok((Category::Fragment, fixture(name)?, Vec::new())),
        ["script"] => Ok((Category::Script, None, Vec::new())),
        ["script", name] => Ok((Category::Script, fixture(name)?, Vec::new())),
        ["pseudocode"] => Ok((Category::Pseudocode, None, Vec::new())),
        ["compile-fail", codes @ ..] if !codes.is_empty() => {
            let mut sorted: Vec<String> = Vec::new();
            for &code in codes {
                if !REGISTRY.split(' ').any(|known| known == code) {
                    return Err(format!("`{code}` is not an error code of the registry (22.2)"));
                }
                if sorted.iter().any(|c| c == code) {
                    return Err(format!("`{code}` is written twice"));
                }
                sorted.push(code.to_string());
            }
            sorted.sort();
            Ok((Category::CompileFail, None, sorted))
        }
        _ => Err(unknown()),
    }
}

/// The fixtures of the manifest (Appendix C).
struct Manifest<'a> {
    /// The declarations of the modules of `std` it provides, by module.
    universe: Vec<(&'a str, &'a str)>,
    /// Each fixture: its name, what it extends and its own lines.
    fixtures: Vec<(&'a str, Option<&'a str>, Vec<&'a str>)>,
    /// Other names of fixtures.
    aliases: Vec<(&'a str, &'a str)>,
}

impl<'a> Manifest<'a> {
    fn parse(text: &'a str) -> Manifest<'a> {
        let mut manifest = Manifest { universe: Vec::new(), fixtures: Vec::new(), aliases: Vec::new() };
        for line in text.lines() {
            let words: Vec<&str> = line.splitn(4, ' ').collect();
            match words.as_slice() {
                ["universe", "decl", module, decl] => manifest.universe.push((module, decl)),
                ["fixture", name, "extends", base] => manifest.fixtures.push((name, Some(base), Vec::new())),
                ["fixture", name] => manifest.fixtures.push((name, None, Vec::new())),
                ["alias", "fixture", name, rest] => manifest.aliases.push((name, rest.trim_start_matches("= "))),
                ["decl" | "binding" | "require", name, ..] => {
                    if let Some(fixture) = manifest.fixtures.iter_mut().find(|f| f.0 == *name) {
                        fixture.2.push(line);
                    }
                }
                _ => {}
            }
        }
        manifest
    }

    /// The lines of a fixture and of those it extends, the base first.
    fn chain(&self, name: &str) -> Option<Vec<&'a str>> {
        let name = self.aliases.iter().find(|(alias, _)| *alias == name).map_or(name, |(_, target)| target);
        let (_, base, lines) = self.fixtures.iter().find(|f| f.0 == name)?;
        let mut all = match base {
            Some(base) => self.chain(base)?,
            None => Vec::new(),
        };
        all.extend(lines);
        Some(all)
    }

    /// What a fixture adds at the end of an example, and its modules of
    /// `std`, by name with their sources.
    fn generate(&self, name: &str) -> (String, Vec<(String, String)>) {
        let lines = self.chain(name).unwrap_or_default();
        let mut imports: Vec<String> = Vec::new();
        let mut items: Vec<String> = Vec::new();
        for line in lines {
            let words: Vec<&str> = line.splitn(3, ' ').collect();
            match words.as_slice() {
                ["binding", _, binding] => {
                    let (alias, module) = binding.split_once(" = ").unwrap_or((binding, binding));
                    imports.push(format!("import {module} as {alias}"));
                }
                ["decl", _, decl] => {
                    let decl = canonical(decl, &mut imports);
                    items.extend(stub(&decl, false));
                }
                _ => {}
            }
        }
        imports.dedup();
        let tail = format!("{}\n\n{}\n", imports.join("\n"), items.join("\n\n"));
        let mut universe: Vec<(String, String)> = Vec::new();
        for (module, decl) in &self.universe {
            let module = module.trim_start_matches("std.").to_string();
            if OWN.contains(&module.as_str()) {
                continue;
            }
            let text = stub(decl, true).unwrap_or_default();
            match universe.iter_mut().find(|(m, _)| *m == module) {
                Some((_, source)) => source.push_str(&format!("\n{text}\n")),
                None => universe.push((module, format!("{text}\n"))),
            }
        }
        (tail, universe)
    }
}

/// A declaration that names a module by its canonical path, `std.json.X`,
/// names it through an import of its own.
fn canonical(decl: &str, imports: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut rest = decl;
    while let Some(at) = rest.find("std.") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 4..];
        let module: String = after.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        imports.push(format!("import std.{module} as __{module}"));
        out.push_str(&format!("__{module}"));
        rest = &after[module.len()..];
    }
    out.push_str(rest);
    out
}

/// A declaration of the manifest as Tondo: an opaque type as a record
/// without fields, or one that holds a `Join` when it is `Send` but neither
/// `Copy` nor `Share`; an operation with a body that is never run. A type
/// the prelude has is the toolchain's, and so are the operations of it
/// that the toolchain has (see the checker).
fn stub(decl: &str, public: bool) -> Option<String> {
    let public = if public { "pub " } else { "" };
    let body = "{\n    panic(\"a fixture is never run\")\n}";
    if let Some(rest) = decl.strip_prefix("opaque ") {
        let (head, capabilities) = match rest.split_once(" capabilities") {
            Some((head, capabilities)) => (head, Some(capabilities)),
            None => (rest, None),
        };
        let name = head.split('[').next().unwrap_or(head);
        if crate::check::PRELUDE_TYPES.contains(&name) {
            return None;
        }
        let fields = match capabilities {
            Some(list) if !list.split(' ').any(|c| c == "Copy") => "{ priv held: Join[Unit] }",
            _ => "{}",
        };
        return Some(format!("{public}type {head} = {fields}"));
    }
    if decl.starts_with("type ") || decl.starts_with("alias ") {
        return Some(format!("{public}{decl}"));
    }
    if let Some(rest) = decl.strip_prefix("impl ") {
        // `impl Display for T` stands for its ordinary implementation (C.4).
        let (header, method) = match rest.split_once(" fn ") {
            Some((header, method)) => (header, format!("fn {method}")),
            None if rest.starts_with("Display ") => (rest, "fn display(self): String".to_string()),
            None => return None,
        };
        return Some(format!("impl {header} {{\n    {method} {}\n}}", body.replace('\n', "\n    ")));
    }
    decl.strip_prefix("unsafe ").unwrap_or(decl).starts_with("fn ").then(|| format!("{public}{decl} {body}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_registry_is_the_specifications() {
        let spec = include_str!("../../../spec/TONDO_LANGUAGE_SPEC.md");
        let start = spec.find("### 22.2").unwrap();
        let end = spec[start..].find("### 22.3").unwrap() + start;
        let codes: Vec<&str> =
            spec[start..end].lines().filter_map(|l| l.strip_prefix("| `E")).map(|l| &l[..4]).collect();
        let registry: Vec<String> = super::REGISTRY.split(' ').map(|c| c[1..].to_string()).collect();
        assert_eq!(registry, codes);
    }
}
