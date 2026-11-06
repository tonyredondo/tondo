//! The two lints a program asks for (language 22.8), which say more than a
//! program needs to compile: `--docs` what a package exports without saying
//! what it is, and `--performance` a copy a loop makes on every turn.

use crate::ast::{self, Block, ExprKind, Item, StmtKind};
use crate::diag::Diagnostics;
use crate::source::{SourceFile, SourceMap};

/// The codes only a check that asks for its profile gives.
const PROFILES: [&str; 2] = ["W1009", "W1010"];

/// `// tondo: allow W1004: why`: takes the warnings of a code on the line the
/// comment is on, or on the next one with code, out of the list. It has to say
/// why, and to silence something: a program with errors may not have reached
/// the warning, and a profile nobody asked for gives none.
pub(crate) fn allow(sources: &SourceMap, diags: &mut Diagnostics) {
    diags.list.retain(|diagnostic| diagnostic.code != Some("W1019"));
    // The standard library says why too, for the test that looks at it.
    let own = std::env::var_os("TONDO_STD_WARNINGS").is_some();
    let foreign = |file: &SourceFile| {
        file.dependency || (file.logical.starts_with('<') && !(own && file.logical.starts_with("<std>")))
    };
    for file in sources.files().filter(|file| !foreign(file) && file.text.contains("tondo: allow")) {
        let (_, comments) = crate::lexer::lex_with_comments(file, None, &mut Diagnostics::default());
        for comment in comments {
            let text = file.slice(comment).trim_start_matches('/').trim();
            let Some(rest) = text.strip_prefix("tondo: allow") else { continue };
            let (codes, why) = rest.split_once(':').unwrap_or((rest, ""));
            let codes: Vec<&str> = codes.split_whitespace().collect();
            let known = |code: &&str| code.starts_with('W') && crate::explain::CODES.iter().any(|c| c.0 == *code);
            if why.trim().is_empty() || codes.is_empty() || !codes.iter().all(known) {
                let message =
                    "this comment takes the codes of warnings and why: `// tondo: allow W1004: the name C gives it`";
                diags.error("E0007", comment, message);
                continue;
            }
            let line = target(file, comment);
            let here = |diagnostic: &crate::diag::Diagnostic, code: &str| {
                diagnostic.code == Some(code)
                    && diagnostic.span.is_some_and(|span| {
                        sources.file_of(span.start).base == file.base && file.line_col(span.start).0 == line
                    })
            };
            for code in codes {
                let (silenced, kept): (Vec<_>, Vec<_>) =
                    std::mem::take(&mut diags.list).into_iter().partition(|d| here(d, code));
                diags.list = kept;
                diags.silenced.extend(silenced);
                let asked = !PROFILES.contains(&code) || diags.asked.contains(&code);
                if asked && !diags.silenced.iter().any(|d| here(d, code)) {
                    diags.warning("W1019", comment, format!("nothing here gives `{code}`: remove this comment"));
                }
            }
        }
    }
    if diags.has_errors() {
        diags.list.retain(|diagnostic| diagnostic.code != Some("W1019"));
    }
}

/// The line a comment speaks of: its own when it follows code, or else the
/// next one with code.
fn target(file: &SourceFile, comment: crate::source::Span) -> usize {
    let (line, column) = file.line_col(comment.start);
    if file.line_text(line).chars().take(column - 1).any(|c| !c.is_whitespace()) {
        return line;
    }
    let code = |at: &usize| {
        let text = file.line_text(*at).trim();
        !text.is_empty() && !text.starts_with("//")
    };
    (line + 1..=file.text.lines().count()).find(code).unwrap_or(line)
}

/// What `tondo check` adds to a program's diagnostics.
pub fn run(sources: &SourceMap, diags: &mut Diagnostics, docs: bool, performance: bool) {
    for file in (0..sources.len()).map(|index| sources.file(index)) {
        // The program's, whether a project's or a file alone.
        if file.dependency || file.logical.starts_with("<std>") || file.name.ends_with("_test.to") {
            continue;
        }
        let module = crate::parser::parse_file(file, &mut Diagnostics::default());
        for item in &module.items {
            if docs {
                undocumented(file, item, diags);
            }
            if let (true, Item::Fn(f)) = (performance, item)
                && let Some(body) = &f.body
            {
                copies(file, body, diags);
            }
        }
    }
    diags.asked = PROFILES.iter().copied().filter(|code| if *code == "W1010" { docs } else { performance }).collect();
    diags.sort(sources);
}

/// W1010: a `pub` declaration with no `///` above it (22.8).
fn undocumented(file: &SourceFile, item: &Item, diags: &mut Diagnostics) {
    let Some(name) = crate::modules::public_name(item) else { return };
    let before = &file.text[..(name.span.start - file.base) as usize];
    // A header may take lines: what counts is above its `pub`, past its annotations.
    let head = before.rfind("\npub ").map_or(0, |at| at + 1);
    let above = before[..head].lines().rev().find(|line| !line.trim_start().starts_with('@'));
    if !above.is_some_and(|line| line.trim_start().starts_with("///")) {
        let message =
            format!("`{}` is public and says nothing of what it is: write a `///` comment above it", name.name);
        diags.warning("W1010", name.span, message);
    }
}

/// W1009 in a block and the blocks in it.
fn copies(file: &SourceFile, block: &Block, diags: &mut Diagnostics) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::For { body, .. } => {
                turn(file, body, diags);
                copies(file, body, diags);
            }
            StmtKind::Expr(value) => {
                if let ExprKind::If { then, .. } = &value.kind {
                    copies(file, then, diags);
                }
            }
            _ => {}
        }
    }
}

/// A body of a loop that names a collection again and then writes it: while
/// the name holds it, the write copies it all, on every turn. Only when nothing
/// reads that name after the write, so that reading the collection itself
/// keeps the order, the panics and the effects of the loop.
fn turn(file: &SourceFile, body: &Block, diags: &mut Diagnostics) {
    for (at, stmt) in body.stmts.iter().enumerate() {
        let StmtKind::Let { pattern, value: Some(value), .. } = &stmt.kind else { continue };
        let (ast::PatternKind::Binding(alias), ExprKind::Path(path)) = (&pattern.kind, &value.kind) else { continue };
        let [owner] = path.as_slice() else { continue };
        let later = &body.stmts[at + 1..];
        let tail = body.tail.as_deref();
        let names_it = |span| names(file, span, &alias.name);
        let read_after = match later.iter().position(|stmt| writes(stmt, &owner.name)) {
            Some(write) => {
                later[write + 1..].iter().any(|stmt| names_it(stmt.span)) || tail.is_some_and(|t| names_it(t.span))
            }
            None if tail.is_some_and(|tail| changes(tail, &owner.name)) => false,
            None => true,
        };
        if read_after {
            continue;
        }
        let message = format!(
            "`{0}` shares `{1}`, so writing `{1}` below copies all of it on every turn: read `{1}` itself, without `{0}`",
            alias.name, owner.name
        );
        diags.warning("W1009", stmt.span, message);
    }
}

/// Whether a statement changes the collection `owner`.
fn writes(stmt: &ast::Stmt, owner: &str) -> bool {
    match &stmt.kind {
        StmtKind::Assign { target, .. } => {
            matches!(&target.kind, ExprKind::Index { base, .. } if is_named(base, owner))
        }
        StmtKind::Expr(value) => changes(value, owner),
        _ => false,
    }
}

/// Whether an expression is a call that changes the collection `owner`.
fn changes(value: &ast::Expr, owner: &str) -> bool {
    let ExprKind::Call { callee, .. } = &value.kind else { return false };
    let ExprKind::Path(path) = &callee.kind else { return false };
    let [named, method] = path.as_slice() else { return false };
    named.name == owner && matches!(method.name.as_str(), "push" | "pop" | "insert" | "remove" | "clear" | "sort")
}

fn is_named(value: &ast::Expr, name: &str) -> bool {
    matches!(&value.kind, ExprKind::Path(path) if path.len() == 1 && path[0].name == name)
}

/// Whether the text of a span names a binding.
pub(crate) fn names(file: &SourceFile, span: crate::source::Span, name: &str) -> bool {
    file.slice(span).split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|word| word == name)
}
