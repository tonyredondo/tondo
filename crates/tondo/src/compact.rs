//! `tondo fmt --compact` (toolchain 7): a file with the fewest characters
//! that keep its tokens — no indentation, spaces only where two tokens
//! would merge, a line per logical line except right after `{` — for giving
//! code to a model with
//! fewer tokens. `tondo fmt` makes it canonical again.

use crate::diag::Diagnostics;
use crate::lexer::{self, Tok, Token};
use crate::source::{SourceFile, SourceMap, Span};
use std::collections::HashMap;
use std::path::Path;

fn lexed(text: &str) -> (Vec<Token>, Vec<Span>, u32) {
    let mut sources = SourceMap::default();
    let id = sources.add(Path::new("compact.to"), text.to_string());
    let file = sources.file(id);
    let (tokens, comments) = lexer::lex_with_comments(file, None, &mut Diagnostics::default());
    (tokens, comments, file.base)
}

/// What the parser reads: the tokens, a string by its text, without the
/// newlines a body may start with.
fn significant(text: &str) -> Vec<String> {
    let (tokens, _, base) = lexed(text);
    let mut out: Vec<String> = Vec::new();
    for t in &tokens {
        let shown = match &t.tok {
            Tok::Str(_) => text[(t.span.start - base) as usize..(t.span.end - base) as usize].to_string(),
            Tok::Eof => continue,
            other => format!("{other:?}"),
        };
        if t.tok == Tok::Newline && matches!(out.last().map(String::as_str), Some("LBrace") | Some("Newline") | None) {
            continue;
        }
        out.push(shown);
    }
    while out.last().map(String::as_str) == Some("Newline") {
        out.pop();
    }
    out
}

/// The compact form of a file, or `None` when it does not format.
pub fn compact(file: &SourceFile, diags: &mut Diagnostics) -> Option<String> {
    let text = crate::fmt::format(file, diags)?;
    let (tokens, comments, base) = lexed(&text);
    let slice = |span: Span| &text[(span.start - base) as usize..(span.end - base) as usize];
    let mut joined: HashMap<(String, String), bool> = HashMap::new();
    // Whether two tokens written together stay those two tokens.
    let mut joins = |a: &str, b: &str| -> bool {
        *joined.entry((a.to_string(), b.to_string())).or_insert_with(|| {
            let kinds = |t: &str| -> Vec<String> {
                lexed(t)
                    .0
                    .into_iter()
                    .filter(|t| !matches!(t.tok, Tok::Newline | Tok::Eof))
                    .map(|t| if matches!(t.tok, Tok::Str(_)) { "Str".into() } else { format!("{:?}", t.tok) })
                    .collect()
            };
            let mut apart = kinds(a);
            apart.extend(kinds(b));
            kinds(&format!("{a}{b}")) == apart
        })
    };
    let mut out = String::new();
    let mut previous: Option<&str> = None;
    let mut newline = false;
    let mut comment = comments.iter().peekable();
    for token in &tokens {
        match token.tok {
            Tok::Newline => {
                newline = true;
                continue;
            }
            Tok::Eof => break,
            _ => {}
        }
        let current = slice(token.span);
        let mut separated = false;
        // A body may start on the line of its `{`.
        if newline && previous.is_some_and(|p| p != "{") {
            out.push('\n');
            separated = true;
        }
        newline = false;
        while comment.peek().is_some_and(|c| c.start < token.span.start) {
            if !separated && previous.is_some() {
                out.push('\n');
            }
            out.push_str(slice(*comment.next().unwrap()).trim_end());
            out.push('\n');
            separated = true;
        }
        if !separated
            && let Some(before) = previous
            && !joins(before, current)
        {
            out.push(' ');
        }
        out.push_str(current);
        previous = Some(current);
    }
    for rest in comment {
        out.push('\n');
        out.push_str(slice(*rest).trim_end());
    }
    out.push('\n');
    // A comment that moved a newline must not change what the parser reads.
    (significant(&out) == significant(&text)).then_some(out).or(Some(text))
}
