//! The two lints a program asks for (language 22.8), which say more than a
//! program needs to compile: `--docs` what a package exports without saying
//! what it is, and `--performance` a copy a loop makes on every turn.

use crate::ast::{self, Block, ExprKind, Item, StmtKind};
use crate::diag::Diagnostics;
use crate::source::{SourceFile, SourceMap};

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
