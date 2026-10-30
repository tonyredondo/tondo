//! `tondo api` (toolchain 7.2): the declarations of a module with their
//! documentation and without bodies, what a reader needs to call it. A
//! module of the standard library that the compiler implements has its
//! declarations written in `std/api`.

use crate::ast;
use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use std::path::Path;

/// The declarations of the standard modules the compiler implements.
const INTRINSIC: &[(&str, &str)] = &[
    ("collections", include_str!("../std/api/collections.to")),
    ("console", include_str!("../std/api/console.to")),
    ("env", include_str!("../std/api/env.to")),
    ("fs", include_str!("../std/api/fs.to")),
    ("io", include_str!("../std/api/io.to")),
    ("path", include_str!("../std/api/path.to")),
    ("process", include_str!("../std/api/process.to")),
    ("sync", include_str!("../std/api/sync.to")),
    ("testing", include_str!("../std/api/testing.to")),
    ("text", include_str!("../std/api/text.to")),
    ("time", include_str!("../std/api/time.to")),
];

/// The methods of the intrinsic types the compiler gives, after the
/// prelude's.
const BUILTINS: &str = include_str!("../std/api/builtins.to");

/// The standard modules written in Tondo that a program imports, without
/// the ones only the toolchain uses.
const HIDDEN: &[&str] = &[
    "datetime",
    "zoned",
    "timetext",
    "files",
    "paths",
    "streams",
    "texts",
    "environment",
    "formats",
    "shapes",
    "fuzz",
    "panics",
];

/// The methods `std/api/builtins.to` gives `owner`, such as `String`, and
/// the operations of the numbers (language 18.9).
pub fn builtin_methods(owner: &str) -> Vec<&'static str> {
    BUILTINS
        .lines()
        .chain(crate::numbers::declarations().lines())
        .filter_map(|line| {
            let rest = line.strip_prefix("pub fn ")?.strip_prefix(owner)?;
            let rest = match rest.strip_prefix('[') {
                Some(generic) => &generic[generic.find(']')? + 1..],
                None => rest,
            };
            let rest = rest.strip_prefix('.')?;
            Some(&rest[..rest.find(['(', '['])?])
        })
        .collect()
}

/// Whether a function's declaration takes `self`, as a method of a value
/// does: `fn Array.length(self): Int`, not `fn String.fromBytes(...)`.
pub fn takes_self(declaration: &str) -> bool {
    let Some(params) = parameters(declaration) else { return false };
    let first = params.trim_start();
    let first = first.strip_prefix("mut ").or_else(|| first.strip_prefix("var ")).unwrap_or(first);
    first.strip_prefix("self").is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// A function declaration from the `(` of its parameters on: what follows
/// its name and type parameters.
pub fn parameters(declaration: &str) -> Option<&str> {
    let mut depth = 0;
    for (at, c) in declaration.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '(' if depth == 0 => return Some(&declaration[at + 1..]),
            _ => {}
        }
    }
    None
}

/// The declaration and documentation of a method the runtime implements,
/// as `tondo api` shows them: `pub fn Array.length(self): Int`.
pub fn builtin_doc(owner: &str, name: &str) -> Option<(String, String)> {
    let wanted = |line: &str| {
        line.strip_prefix("pub fn ").and_then(|rest| rest.strip_prefix(owner)).is_some_and(|rest| {
            let rest = rest.strip_prefix('[').map_or(rest, |g| g.find(']').map_or(g, |end| &g[end + 1..]));
            rest.strip_prefix('.')
                .is_some_and(|rest| rest.starts_with(name) && rest[name.len()..].starts_with(['(', '[']))
        })
    };
    declaration(BUILTINS, wanted).or_else(|| declaration(crate::numbers::declarations(), wanted))
}

/// The function `name` of a standard module, as `tondo api` shows it:
/// `pub fn println(text: String)` of `std.console`, and its documentation.
pub fn function_doc(module: &str, name: &str) -> Option<(String, String)> {
    let text = standard(module)?;
    declaration(&text, |line| {
        line.strip_prefix("pub fn ")
            .is_some_and(|rest| rest.starts_with(name) && rest[name.len()..].starts_with(['(', '[']))
    })
}

/// The first line of `text` that `wanted` accepts, and the `///` lines
/// above it.
fn declaration(text: &str, wanted: impl Fn(&str) -> bool) -> Option<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let found = lines.iter().position(|line| wanted(line))?;
    let docs: Vec<&str> = lines[..found]
        .iter()
        .rev()
        .take_while(|line| line.starts_with("///"))
        .map(|line| line.trim_start_matches("///").trim_start())
        .collect();
    let docs: Vec<&str> = docs.into_iter().rev().collect();
    Some((lines[found].to_string(), docs.join("\n")))
}

/// The names of the standard modules, as `std.name`, and `prelude`.
pub fn standard_modules() -> Vec<String> {
    let mut names: Vec<String> = INTRINSIC.iter().map(|(n, _)| format!("std.{n}")).collect();
    for name in crate::stdlib::names() {
        if name == "prelude" {
            names.push("prelude".into());
        } else if !HIDDEN.contains(&name) {
            names.push(format!("std.{name}"));
        }
    }
    names.sort();
    names
}

/// What a standard module is for, from the comment at its top: `the
/// filesystem. Operations wait without blocking a worker.`
pub fn summary(module: &str) -> Option<String> {
    let name = module.strip_prefix("std.")?;
    let source = match INTRINSIC.iter().find(|(n, _)| *n == name) {
        Some((_, text)) => text,
        None => crate::stdlib::text(name).filter(|_| !HIDDEN.contains(&name))?,
    };
    let lines: Vec<&str> = source.lines().map_while(|line| line.strip_prefix("// ")).collect();
    let mut text = lines.join(" ");
    // Where the specification has it is for its readers.
    while let Some(start) = text.find(" (standard library ") {
        let Some(end) = text[start..].find(')') else { break };
        text.replace_range(start..start + end + 1, "");
    }
    let text = text.strip_prefix(module)?.strip_prefix(": ")?;
    Some(text.to_string())
}

/// The API of a standard module: `std.http`, `std.fs` or `prelude`.
pub fn standard(name: &str) -> Option<String> {
    if name == "prelude" {
        let source = crate::stdlib::text("prelude")?;
        // `Panic` is the prelude's, from a module of the toolchain: its
        // type, its method and its `Display`, in place of the alias.
        let panics = render(crate::stdlib::text("panics")?, false);
        let panic: Vec<&str> = panics
            .trim_end()
            .split("\n\n")
            .filter(|part| {
                part.lines().any(|line| {
                    ["pub type Panic ", "pub fn Panic.", "impl Display for Panic"].iter().any(|p| line.starts_with(p))
                })
            })
            .collect();
        let prelude: Vec<String> = render(source, false)
            .trim_end()
            .split("\n\n")
            .map(|part| match part.lines().any(|line| line.starts_with("pub alias Panic ")) {
                true => panic.join("\n\n"),
                false => part.to_string(),
            })
            .collect();
        let numbers = crate::numbers::declarations();
        return Some(format!("{}\n\n{BUILTINS}\n{numbers}", prelude.join("\n\n")));
    }
    let name = name.strip_prefix("std.")?;
    if let Some((_, text)) = INTRINSIC.iter().find(|(n, _)| *n == name) {
        // With the declarations of its parts written in Tondo.
        let mut text = text.to_string();
        for companion in crate::stdlib::companions(name) {
            let source = crate::stdlib::text(companion)?.replace(&format!("__{name}."), "");
            let rendered = render(&source, false);
            // Without the comment at its top, which says what the part is.
            let lines: Vec<&str> = rendered
                .lines()
                .skip_while(|line| line.is_empty() || (line.starts_with("//") && !line.starts_with("///")))
                .collect();
            let declarations = lines.join("\n");
            let declarations = declarations.trim_end();
            if !declarations.is_empty() {
                text = format!("{}\n\n{declarations}\n", text.trim_end());
            }
        }
        return Some(text);
    }
    crate::stdlib::text(name).filter(|_| !HIDDEN.contains(&name)).map(|source| render(source, false))
}

/// The declarations of a module's source: the public ones, or every one
/// with `private`. A module's imports under another name read as the
/// module's own name: `io.Reader`, not `__io.Reader`.
pub fn render(source: &str, private: bool) -> String {
    let mut sources = SourceMap::default();
    let original = sources.add(Path::new("api.to"), source.to_string());
    let text = crate::fmt::format(sources.file(original), &mut Diagnostics::default()).unwrap_or(source.to_string());
    let id = sources.add(Path::new("api.to"), text.clone());
    let file = sources.file(id);
    let mut diags = Diagnostics::default();
    let module = crate::parser::parse_file(file, &mut diags);
    let (_, comments) = crate::lexer::lex_with_comments(file, None, &mut Diagnostics::default());
    let base = file.base;
    let slice = |span: Span| text[(span.start - base) as usize..(span.end - base) as usize].to_string();
    // The documentation between two offsets: `///` lines, as written.
    let docs = |from: u32, to: u32, indent: &str| -> String {
        comments
            .iter()
            .filter(|c| c.start >= from && c.end <= to)
            .map(|c| slice(*c))
            .filter(|c| c.starts_with("///"))
            .map(|c| format!("{indent}{}\n", c.trim_end()))
            .collect()
    };
    let header = |f: &ast::FnDecl| match &f.body {
        Some(body) => text[(f.span.start - base) as usize..(body.span.start - base) as usize].trim_end().to_string(),
        None => slice(f.span),
    };
    // Up to the `{` that opens a body.
    let opening = |span: Span| {
        let start = (span.start - base) as usize;
        let open = text[start..].find('{').map_or(start, |o| start + o);
        (text[start..open].trim_end().to_string(), open as u32 + base)
    };
    let mut out: Vec<String> = Vec::new();
    let mut previous = base;
    for item in &module.items {
        let (public, span) = match item {
            ast::Item::Fn(f) => (f.public, f.span),
            ast::Item::Type(t) => (t.public, t.span),
            ast::Item::Enum(e) => (e.public, e.span),
            ast::Item::Trait(t) => (t.public, t.span),
            ast::Item::Alias(a) => (a.public, a.span),
            ast::Item::Const(c) => (c.public, c.span),
            ast::Item::Impl(i) => (true, i.span),
            ast::Item::Derive(d) => (true, d.span),
            ast::Item::Test(t) => (false, t.span()),
        };
        let documented = docs(previous, span.start, "");
        previous = span.end;
        if (!public && !private) || matches!(item, ast::Item::Test(_)) {
            continue;
        }
        let declaration = match item {
            ast::Item::Fn(f) => header(f),
            ast::Item::Type(t) => match &t.body {
                ast::TypeBody::Record(fields) if !private && fields.iter().any(|f| f.private) => {
                    let (head, open) = opening(t.span);
                    let mut lines = vec![format!("{head} {{")];
                    let mut from = open;
                    for field in fields {
                        let end = field.ty.span.end;
                        if !field.private {
                            lines.push(format!(
                                "{}    {}: {}",
                                docs(from, field.name.span.start, "    "),
                                field.name.name,
                                slice(field.ty.span)
                            ));
                        }
                        from = end;
                    }
                    lines.push("    // private fields".into());
                    lines.push("}".into());
                    lines.join("\n")
                }
                _ => slice(t.span),
            },
            ast::Item::Trait(t) => {
                let (head, open) = opening(t.span);
                let mut from = open;
                let mut methods = Vec::new();
                for method in &t.methods {
                    methods.push(format!("{}    {}", docs(from, method.span.start, "    "), header(method)));
                    from = method.span.end;
                }
                format!("{head} {{\n{}\n}}", methods.join("\n\n"))
            }
            ast::Item::Impl(i) => opening(i.span).0,
            _ => slice(span),
        };
        out.push(format!("{documented}{declaration}"));
    }
    if out.is_empty() {
        return String::new();
    }
    let mut rendered = out.join("\n\n") + "\n";
    for import in &module.imports {
        let (Some(alias), Some(last)) = (&import.alias, import.path.last()) else { continue };
        if alias.name != last.name && !import.path.iter().any(|p| p.name == "internal") {
            rendered = renamed(&rendered, &alias.name, &last.name);
        }
    }
    rendered
}

/// `alias.` as `name.` where `alias` is a whole word.
fn renamed(text: &str, alias: &str, name: &str) -> String {
    let pattern = format!("{alias}.");
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(&pattern) {
        let whole = rest[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        out.push_str(&rest[..at]);
        out.push_str(if whole { name } else { alias });
        out.push('.');
        rest = &rest[at + pattern.len()..];
    }
    out.push_str(rest);
    out
}

/// The declarations of what `render` shows, as `tondo api --json` gives
/// them: each one's kind, name, text and documentation.
pub fn declarations(text: &str) -> Vec<serde_json::Value> {
    // Blocks end at a blank line outside braces.
    let mut blocks: Vec<Vec<&str>> = vec![Vec::new()];
    let mut depth = 0i32;
    for line in text.lines() {
        if line.trim().is_empty() && depth == 0 {
            blocks.push(Vec::new());
            continue;
        }
        if !line.trim_start().starts_with("//") {
            depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        }
        blocks.last_mut().unwrap().push(line);
    }
    let mut found = Vec::new();
    for block in blocks {
        let doc: Vec<&str> = block
            .iter()
            .take_while(|line| line.starts_with("///"))
            .map(|line| line.trim_start_matches("///").strip_prefix(' ').unwrap_or(&line[3..]))
            .collect();
        let code = block[doc.len()..].join("\n");
        // A comment alone, as a module's header, declares nothing.
        let Some(first) = code.lines().next().filter(|line| !line.starts_with("//")) else { continue };
        let mut head = first.trim_end_matches(" {");
        for word in ["pub ", "unsafe ", "extern "] {
            head = head.strip_prefix(word).unwrap_or(head);
        }
        let kind = head.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or(head);
        let rest = &head[kind.len()..];
        let name = match kind {
            "impl" => skip_brackets(rest).trim().to_string(),
            "fn" => without_brackets(rest.trim().split('(').next().unwrap_or(rest)),
            _ => rest.trim().split(['[', ':', '=', ' ']).next().unwrap_or(rest).to_string(),
        };
        found.push(serde_json::json!({ "kind": kind, "name": name, "declaration": code, "doc": doc.join("\n") }));
    }
    found
}

/// A function's name without its type parameters: `Option.map` for
/// `Option[T].map[U]`.
fn without_brackets(text: &str) -> String {
    let mut depth = 0;
    text.chars()
        .filter(|&c| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => return depth == 0,
            }
            false
        })
        .collect()
}

/// The text after a leading `[...]`, as the parameters of a generic `impl`.
fn skip_brackets(text: &str) -> &str {
    if !text.starts_with('[') {
        return text;
    }
    let mut depth = 0;
    for (index, c) in text.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return &text[index + 1..];
                }
            }
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every public declaration of the standard library says what it does,
    /// which `tondo api` and an editor's completion show.
    #[test]
    fn every_standard_declaration_is_documented() {
        let mut missing = Vec::new();
        for module in standard_modules() {
            let text = standard(&module).expect("a standard module");
            for declared in declarations(&text) {
                let kind = declared["kind"].as_str().unwrap_or_default();
                let documented = !declared["doc"].as_str().unwrap_or_default().is_empty();
                if matches!(kind, "fn" | "type" | "enum" | "trait" | "const" | "alias") && !documented {
                    missing.push(format!("{module}: {}", declared["name"]));
                }
            }
        }
        assert!(missing.is_empty(), "write a `///` line above each of these:\n{}", missing.join("\n"));
    }
}
