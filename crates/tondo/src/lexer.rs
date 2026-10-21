//! Lexer (spec 5 and 23.2–23.3).
//!
//! Lexing runs in two passes: the first produces raw tokens with one marker
//! per run of physical newlines, the second applies the closed algorithm of
//! 5.2 to decide which markers become significant `Newline` tokens.

use crate::diag::Diagnostics;
use crate::source::{SourceFile, Span};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Kw(Kw),
    Int { value: u64, suffix: Option<&'static str> },
    Float { text: String, suffix: Option<&'static str> },
    Char(char),
    Str(Vec<StrPart>),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    DotDot,
    DotDotEq,
    Ellipsis,
    Colon,
    FatArrow,
    Eq,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    Bang,
    Question,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,
    Newline,
    Eof,
}

/// A piece of a string literal: decoded text or the source span of an
/// interpolated expression, which the parser lexes and parses on its own.
#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Text(String),
    Expr(Span),
}

macro_rules! keywords {
    ($($name:ident = $text:literal,)*) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub enum Kw { $($name,)* }

        impl Kw {
            pub fn lookup(text: &str) -> Option<Kw> {
                match text { $($text => Some(Kw::$name),)* _ => None }
            }

            pub fn as_str(self) -> &'static str {
                match self { $(Kw::$name => $text,)* }
            }
        }
    };
}

keywords! {
    Alias = "alias", And = "and", As = "as", Break = "break",
    Const = "const", Continue = "continue", Defer = "defer", Derive = "derive",
    Else = "else", Enum = "enum", Fail = "fail", False = "false",
    Fn = "fn", For = "for", If = "if", Impl = "impl", Import = "import", In = "in",
    Let = "let", Match = "match", Mut = "mut", None = "none", Not = "not",
    Or = "or", Priv = "priv", Pub = "pub", Ref = "ref", Return = "return",
    Select = "select", SelfValue = "self", Spawn = "spawn", Suite = "suite",
    Test = "test", Trait = "trait", True = "true", Type = "type",
    Unsafe = "unsafe", Var = "var", With = "with",
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("identifier `{name}`"),
            Tok::Kw(kw) => format!("`{}`", kw.as_str()),
            Tok::Int { .. } | Tok::Float { .. } => "number".into(),
            Tok::Char(_) => "character literal".into(),
            Tok::Str(_) => "string literal".into(),
            Tok::Newline => "end of line".into(),
            Tok::Eof => "end of file".into(),
            other => format!("`{}`", punct_text(other)),
        }
    }
}

fn punct_text(tok: &Tok) -> &'static str {
    PUNCTUATION.iter().find(|(_, t)| t == tok).map_or("?", |(text, _)| text)
}

/// Longest spellings first, so a linear scan implements maximal munch.
const PUNCTUATION: &[(&str, Tok)] = &[
    ("...", Tok::Ellipsis),
    ("..=", Tok::DotDotEq),
    ("<<=", Tok::ShlEq),
    (">>=", Tok::ShrEq),
    ("..", Tok::DotDot),
    ("=>", Tok::FatArrow),
    ("==", Tok::EqEq),
    ("!=", Tok::NotEq),
    ("<=", Tok::Le),
    (">=", Tok::Ge),
    ("<<", Tok::Shl),
    (">>", Tok::Shr),
    ("+=", Tok::PlusEq),
    ("-=", Tok::MinusEq),
    ("*=", Tok::StarEq),
    ("/=", Tok::SlashEq),
    ("%=", Tok::PercentEq),
    ("&=", Tok::AmpEq),
    ("|=", Tok::PipeEq),
    ("^=", Tok::CaretEq),
    ("(", Tok::LParen),
    (")", Tok::RParen),
    ("[", Tok::LBracket),
    ("]", Tok::RBracket),
    ("{", Tok::LBrace),
    ("}", Tok::RBrace),
    (",", Tok::Comma),
    (".", Tok::Dot),
    (":", Tok::Colon),
    ("=", Tok::Eq),
    ("<", Tok::Lt),
    (">", Tok::Gt),
    ("+", Tok::Plus),
    ("-", Tok::Minus),
    ("*", Tok::Star),
    ("/", Tok::Slash),
    ("%", Tok::Percent),
    ("&", Tok::Amp),
    ("|", Tok::Pipe),
    ("^", Tok::Caret),
    ("~", Tok::Tilde),
    ("!", Tok::Bang),
    ("?", Tok::Question),
];

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// Binary operators that continue an expression across a newline (5.2).
fn is_binary_continuation(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Plus
            | Tok::Minus
            | Tok::Star
            | Tok::Slash
            | Tok::Percent
            | Tok::Shl
            | Tok::Shr
            | Tok::Amp
            | Tok::Caret
            | Tok::Pipe
            | Tok::DotDot
            | Tok::DotDotEq
            | Tok::Lt
            | Tok::Le
            | Tok::Gt
            | Tok::Ge
            | Tok::EqEq
            | Tok::NotEq
            | Tok::Kw(Kw::In | Kw::And | Kw::Or | Kw::With)
    )
}

/// Tokens after which a newline never ends a line (5.2).
fn suppresses_following_newline(tok: &Tok) -> bool {
    is_binary_continuation(tok)
        || matches!(
            tok,
            Tok::Comma
                | Tok::Dot
                | Tok::Colon
                | Tok::FatArrow
                | Tok::Bang
                | Tok::Eq
                | Tok::PlusEq
                | Tok::MinusEq
                | Tok::StarEq
                | Tok::SlashEq
                | Tok::PercentEq
                | Tok::AmpEq
                | Tok::PipeEq
                | Tok::CaretEq
                | Tok::ShlEq
                | Tok::ShrEq
                | Tok::Tilde
                | Tok::Kw(Kw::Not | Kw::Spawn | Kw::Fail | Kw::Defer)
        )
}

const INT_SUFFIXES: [&str; 8] = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"];
const FLOAT_SUFFIXES: [&str; 2] = ["f32", "f64"];

/// Lexes a whole file, or the source range of an interpolated expression.
pub fn lex(file: &SourceFile, range: Option<Span>, diags: &mut Diagnostics) -> Vec<Token> {
    lex_with_comments(file, range, diags).0
}

/// The tokens and the spans of the comments, which the formatter keeps.
pub fn lex_with_comments(file: &SourceFile, range: Option<Span>, diags: &mut Diagnostics) -> (Vec<Token>, Vec<Span>) {
    let (start, end) = match range {
        Some(span) => ((span.start - file.base) as usize, (span.end - file.base) as usize),
        None => (0, file.text.len()),
    };
    let mut lexer = Lexer {
        text: &file.text,
        base: file.base,
        pos: start,
        end,
        diags,
        raw: Vec::new(),
        delims: Vec::new(),
        comments: Vec::new(),
    };
    if range.is_none() && file.text.starts_with("#!") {
        lexer.pos = file.text.find('\n').unwrap_or(file.text.len());
    }
    lexer.run();
    let comments = lexer.comments;
    (significant_newlines(lexer.raw, file.base + end as u32), comments)
}

enum Raw {
    Token(Token),
    /// A run of physical newlines; `bracketed` when `(` or `[` is the
    /// innermost open delimiter.
    Newline {
        bracketed: bool,
        span: Span,
    },
}

fn significant_newlines(raw: Vec<Raw>, end: u32) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(raw.len());
    for (i, item) in raw.iter().enumerate() {
        match item {
            Raw::Token(token) => out.push(token.clone()),
            Raw::Newline { bracketed, span } => {
                if *bracketed {
                    continue;
                }
                let previous = out.last().map(|t| &t.tok);
                if previous.is_none_or(|t| *t == Tok::Newline || suppresses_following_newline(t)) {
                    continue;
                }
                let next = raw[i + 1..].iter().find_map(|r| match r {
                    Raw::Token(t) => Some(&t.tok),
                    Raw::Newline { .. } => None,
                });
                if next.is_some_and(|t| *t == Tok::Dot || is_binary_continuation(t)) {
                    continue;
                }
                out.push(Token { tok: Tok::Newline, span: *span });
            }
        }
    }
    if out.last().is_some_and(|t| t.tok != Tok::Newline) {
        out.push(Token { tok: Tok::Newline, span: Span::new(end, end) });
    }
    out.push(Token { tok: Tok::Eof, span: Span::new(end, end) });
    out
}

struct Lexer<'a> {
    text: &'a str,
    base: u32,
    pos: usize,
    end: usize,
    diags: &'a mut Diagnostics,
    raw: Vec<Raw>,
    delims: Vec<Tok>,
    comments: Vec<Span>,
}

impl Lexer<'_> {
    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.base + start as u32, self.base + end as u32)
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..self.end].chars().next()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.text[self.pos..self.end].chars().nth(offset)
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..self.end]
    }

    fn error(&mut self, code: &'static str, start: usize, end: usize, message: impl Into<String>) {
        let span = self.span(start, end);
        self.diags.error(code, span, message);
    }

    fn push(&mut self, tok: Tok, start: usize) {
        let span = self.span(start, self.pos);
        self.raw.push(Raw::Token(Token { tok, span }));
    }

    fn newline(&mut self, start: usize) {
        if let Some(Raw::Newline { span, .. }) = self.raw.last_mut() {
            span.end = self.base + self.pos as u32;
            return;
        }
        let bracketed = matches!(self.delims.last(), Some(Tok::LParen | Tok::LBracket));
        let span = self.span(start, self.pos);
        self.raw.push(Raw::Newline { bracketed, span });
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                ' ' | '\t' => self.pos += 1,
                '\n' => {
                    self.pos += 1;
                    self.newline(start);
                }
                '\r' => {
                    if self.peek_at(1) == Some('\n') {
                        self.pos += 2;
                        self.newline(start);
                    } else {
                        self.pos += 1;
                        self.error("E0002", start, self.pos, "carriage return outside CRLF");
                    }
                }
                '/' if self.rest().starts_with("//") => {
                    self.pos = self.rest().find('\n').map_or(self.end, |i| self.pos + i);
                    let end = if self.text[..self.pos].ends_with('\r') { self.pos - 1 } else { self.pos };
                    self.comments.push(self.span(start, end));
                }
                '/' if self.rest().starts_with("/*") => self.block_comment(),
                '"' => self.string(false),
                'r' if self.rest().starts_with("r\"") => {
                    self.pos += 1;
                    self.string(true);
                }
                '\'' => self.char_literal(),
                '0'..='9' => self.number(),
                c if c == '_' || unicode_ident::is_xid_start(c) => self.identifier(),
                _ => self.punctuation(c),
            }
        }
    }

    fn block_comment(&mut self) {
        let start = self.pos;
        let mut depth = 0;
        let mut has_newline = false;
        while self.pos < self.end {
            let rest = self.rest();
            if rest.starts_with("/*") {
                depth += 1;
                self.pos += 2;
            } else if rest.starts_with("*/") {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    break;
                }
            } else {
                let c = self.peek().unwrap();
                has_newline |= c == '\n';
                self.pos += c.len_utf8();
            }
        }
        if depth != 0 {
            self.error("E0002", start, start + 2, "unterminated block comment");
        }
        self.comments.push(self.span(start, self.pos));
        if has_newline {
            self.newline(start);
        }
    }

    fn identifier(&mut self) {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == '_' || unicode_ident::is_xid_continue(c) {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let name: String = self.text[start..self.pos].nfc().collect();
        let tok = match Kw::lookup(&name) {
            Some(kw) => Tok::Kw(kw),
            None => Tok::Ident(name),
        };
        self.push(tok, start);
    }

    fn punctuation(&mut self, c: char) {
        let start = self.pos;
        let rest = self.rest();
        if rest.starts_with("--") || rest.starts_with("??") {
            self.pos += 2;
            self.error("E0002", start, self.pos, "adjacent `--` and `??` are not Tondo operators");
            return;
        }
        let Some((text, tok)) = PUNCTUATION.iter().find(|(text, _)| rest.starts_with(text)) else {
            self.pos += c.len_utf8();
            let message =
                if c == ';' { "`;` is not a Tondo token".to_string() } else { format!("unexpected character `{c}`") };
            self.error("E0002", start, self.pos, message);
            return;
        };
        self.pos += text.len();
        match tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => self.delims.push(tok.clone()),
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                self.delims.pop();
            }
            _ => {}
        }
        self.push(tok.clone(), start);
    }

    fn digits(&mut self, valid: impl Fn(char) -> bool) -> bool {
        // `_` may only separate two digits.
        let mut ok = matches!(self.peek(), Some(c) if valid(c));
        let mut last_was_digit = false;
        while let Some(c) = self.peek() {
            if valid(c) {
                last_was_digit = true;
            } else if c == '_' {
                ok &= last_was_digit && self.peek_at(1).is_some_and(&valid);
                last_was_digit = false;
            } else {
                break;
            }
            self.pos += 1;
        }
        ok
    }

    fn suffix(&mut self, options: &[&'static str]) -> Option<&'static str> {
        let rest = self.rest();
        let found = options.iter().copied().find(|s| {
            rest.starts_with(s) && !rest[s.len()..].starts_with(|c: char| c == '_' || unicode_ident::is_xid_continue(c))
        })?;
        self.pos += found.len();
        Some(found)
    }

    fn number(&mut self) {
        let start = self.pos;
        let radix = match self.rest().get(..2) {
            Some("0b") => 2,
            Some("0o") => 8,
            Some("0x") => 16,
            _ => 10,
        };
        let mut ok;
        let mut is_float = false;
        if radix != 10 {
            self.pos += 2;
            ok = self.digits(|c| c.is_digit(radix));
        } else {
            ok = self.digits(|c| c.is_ascii_digit());
            let integer = &self.text[start..self.pos];
            ok &= !(integer.len() > 1 && integer.starts_with('0'));
            if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                self.pos += 1;
                ok &= self.digits(|c| c.is_ascii_digit());
            }
            if matches!(self.peek(), Some('e' | 'E')) {
                let save = self.pos;
                self.pos += 1;
                if matches!(self.peek(), Some('+' | '-')) {
                    self.pos += 1;
                }
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    is_float = true;
                    ok &= self.digits(|c| c.is_ascii_digit());
                } else {
                    self.pos = save;
                }
            }
        }
        let digits_end = self.pos;
        let suffix = self.suffix(if is_float { &FLOAT_SUFFIXES } else { &INT_SUFFIXES });
        // An adjacent identifier character makes the whole run malformed.
        while let Some(c) = self.peek() {
            if c == '_' || unicode_ident::is_xid_continue(c) {
                ok = false;
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        if !ok {
            self.error("E0003", start, self.pos, "malformed numeric literal");
            self.push(Tok::Int { value: 0, suffix: None }, start);
            return;
        }
        let digits_start = if radix == 10 { start } else { start + 2 };
        let cleaned: String = self.text[digits_start..digits_end].chars().filter(|&c| c != '_').collect();
        if is_float {
            self.push(Tok::Float { text: cleaned, suffix }, start);
            return;
        }
        match u64::from_str_radix(&cleaned, radix) {
            Ok(value) => self.push(Tok::Int { value, suffix }, start),
            Err(_) => {
                self.error("E0003", start, self.pos, "integer literal is too large");
                self.push(Tok::Int { value: 0, suffix }, start);
            }
        }
    }

    /// Decodes one escape after `\`; `quote` is the escapable delimiter.
    fn escape(&mut self, quote: char) -> Option<char> {
        let start = self.pos;
        self.pos += 1; // the backslash
        let c = self.peek();
        self.pos += c.map_or(0, char::len_utf8);
        let value = match c {
            Some('n') => Some('\n'),
            Some('r') => Some('\r'),
            Some('t') => Some('\t'),
            Some('\\') => Some('\\'),
            Some('0') => Some('\0'),
            Some(q) if q == quote => Some(q),
            Some('u') if self.peek() == Some('{') => {
                let close = self.rest().find('}').map(|i| self.pos + i);
                let value = close.and_then(|close| {
                    let hex = &self.text[self.pos + 1..close];
                    let valid = (1..=6).contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_hexdigit());
                    self.pos = close + 1;
                    valid.then(|| u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)).flatten()
                });
                if value.is_none() {
                    self.error("E0003", start, self.pos, "invalid unicode escape");
                    return Some('\u{FFFD}');
                }
                value
            }
            _ => None,
        };
        if value.is_none() {
            self.error("E0003", start, self.pos, "unknown escape sequence");
        }
        value
    }

    fn char_literal(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let mut chars = Vec::new();
        let mut ok = true;
        loop {
            match self.peek() {
                None | Some('\n' | '\r') => {
                    self.error("E0003", start, self.pos, "unterminated character literal");
                    self.push(Tok::Char('\0'), start);
                    return;
                }
                Some('\'') => {
                    self.pos += 1;
                    break;
                }
                Some('\\') => chars.extend(self.escape('\'')),
                Some(c) => {
                    ok &= !c.is_ascii_control() || c == '\t';
                    chars.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        if !ok || chars.len() != 1 {
            self.error("E0003", start, self.pos, "a character literal contains exactly one Unicode scalar");
        }
        self.push(Tok::Char(chars.first().copied().unwrap_or('\0')), start);
    }

    /// Skips an interpolated expression starting after its `{`; returns the
    /// offset of the matching `}`. Nested braces, strings, chars and
    /// comments do not affect the balance.
    fn skip_interpolation(&mut self, multiline: bool) -> Option<usize> {
        let mut depth = 1;
        while let Some(c) = self.peek() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(self.pos);
                    }
                }
                '\n' if !multiline => return None,
                '"' => {
                    self.skip_nested_string();
                    continue;
                }
                '\'' => {
                    self.pos += 1;
                    while let Some(c) = self.peek() {
                        self.pos += c.len_utf8();
                        match c {
                            '\\' => self.pos += self.peek().map_or(0, char::len_utf8),
                            '\'' | '\n' => break,
                            _ => {}
                        }
                    }
                    continue;
                }
                '/' if self.rest().starts_with("//") => {
                    self.pos = self.rest().find('\n').map_or(self.end, |i| self.pos + i);
                    continue;
                }
                '/' if self.rest().starts_with("/*") => {
                    self.block_comment();
                    continue;
                }
                _ => {}
            }
            self.pos += c.len_utf8();
        }
        None
    }

    fn skip_nested_string(&mut self) {
        let multiline = self.rest().starts_with("\"\"\"");
        self.pos += if multiline { 3 } else { 1 };
        while let Some(c) = self.peek() {
            if multiline && self.rest().starts_with("\"\"\"") {
                self.pos += 3;
                return;
            }
            self.pos += c.len_utf8();
            match c {
                '"' if !multiline => return,
                '\\' => self.pos += self.peek().map_or(0, char::len_utf8),
                '{' if self.peek() == Some('{') => self.pos += 1,
                '{' => {
                    if self.skip_interpolation(multiline).is_some() {
                        self.pos += 1;
                    }
                }
                '\n' if !multiline => return,
                _ => {}
            }
        }
    }

    fn string(&mut self, raw: bool) {
        let start = if raw { self.pos - 1 } else { self.pos };
        let multiline = self.rest().starts_with("\"\"\"");
        let open = if multiline { 3 } else { 1 };
        self.pos += open;
        let content_start = self.pos;

        // First pass: find the closing delimiter.
        let content_end = loop {
            let Some(c) = self.peek() else {
                self.error("E0003", start, self.pos, "unterminated string literal");
                self.push(Tok::Str(Vec::new()), start);
                return;
            };
            if multiline && self.rest().starts_with("\"\"\"") {
                break self.pos;
            }
            match c {
                '"' if !multiline => break self.pos,
                '\n' | '\r' if !multiline => {
                    self.error("E0003", start, self.pos, "unterminated string literal");
                    self.push(Tok::Str(Vec::new()), start);
                    return;
                }
                '\\' if !raw => self.pos += 1 + self.rest()[1..].chars().next().map_or(0, char::len_utf8),
                '{' if !raw && self.peek_at(1) != Some('{') => {
                    self.pos += 1;
                    if self.skip_interpolation(multiline).is_some() {
                        self.pos += 1;
                    }
                }
                '{' if !raw => self.pos += 2,
                _ => self.pos += c.len_utf8(),
            }
        };
        let after = content_end + open;

        // Multiline dedentation (5.10) is computed on the raw source.
        let (mut from, mut to, mut prefix) = (content_start, content_end, "");
        if multiline {
            let content = &self.text[content_start..content_end];
            if content.starts_with("\r\n") {
                from += 2;
            } else if content.starts_with('\n') {
                from += 1;
            }
            if let Some(last_newline) = content.rfind('\n') {
                let tail = &content[last_newline + 1..];
                if content_start + last_newline + 1 >= from && tail.chars().all(|c| c == ' ' || c == '\t') {
                    prefix = tail;
                    to = content_start + last_newline;
                    if self.text[..to].ends_with('\r') {
                        to -= 1;
                    }
                    to = to.max(from);
                }
            }
        }

        // Second pass: decode text and collect interpolations.
        self.pos = from;
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut line_start = multiline;
        while self.pos < to {
            if line_start {
                line_start = false;
                let line_end = self.rest().find('\n').map_or(to, |i| (self.pos + i).min(to));
                let line = &self.text[self.pos..line_end];
                if line.trim_end_matches('\r').chars().all(|c| c == ' ' || c == '\t') {
                    let common = line.bytes().zip(prefix.bytes()).take_while(|(a, b)| a == b).count();
                    self.pos += common;
                } else if line.starts_with(prefix) {
                    self.pos += prefix.len();
                } else {
                    self.error(
                        "E0003",
                        self.pos,
                        line_end,
                        "multiline string line does not start with the closing indentation",
                    );
                }
                continue;
            }
            let c = self.peek().unwrap();
            match c {
                '\r' if self.peek_at(1) == Some('\n') => {
                    text.push('\n');
                    self.pos += 2;
                    line_start = true;
                }
                '\r' => {
                    self.error("E0002", self.pos, self.pos + 1, "carriage return outside CRLF");
                    self.pos += 1;
                }
                '\n' => {
                    text.push('\n');
                    self.pos += 1;
                    line_start = true;
                }
                '\\' if !raw => text.extend(self.escape('"')),
                '{' if !raw && self.peek_at(1) == Some('{') => {
                    text.push('{');
                    self.pos += 2;
                }
                '{' if !raw => {
                    let open_at = self.pos;
                    self.pos += 1;
                    match self.skip_interpolation(multiline) {
                        Some(close) if self.text[open_at + 1..close].trim().is_empty() => {
                            self.error("E0003", open_at, close + 1, "empty interpolation");
                            self.pos = close + 1;
                        }
                        Some(close) => {
                            if !text.is_empty() {
                                parts.push(StrPart::Text(std::mem::take(&mut text)));
                            }
                            parts.push(StrPart::Expr(self.span(open_at + 1, close)));
                            self.pos = close + 1;
                        }
                        None => {
                            self.error("E0003", open_at, open_at + 1, "unterminated interpolation");
                            self.pos = to;
                        }
                    }
                }
                '}' if !raw && self.peek_at(1) == Some('}') => {
                    text.push('}');
                    self.pos += 2;
                }
                '}' if !raw => {
                    self.error("E0003", self.pos, self.pos + 1, "unmatched `}` in string literal; write `}}`");
                    self.pos += 1;
                }
                c if c.is_ascii_control() && c != '\t' && !multiline => {
                    self.error("E0003", self.pos, self.pos + 1, "control character in string literal");
                    self.pos += 1;
                }
                c => {
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        if !text.is_empty() || parts.is_empty() {
            parts.push(StrPart::Text(text));
        }
        self.pos = after;
        self.push(Tok::Str(parts), start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceMap;
    use std::path::Path;

    fn toks(text: &str) -> (Vec<Tok>, usize) {
        let mut sources = SourceMap::default();
        let file = sources.add(Path::new("t.to"), text.into());
        let mut diags = Diagnostics::default();
        let tokens = lex(sources.file(file), None, &mut diags);
        (tokens.into_iter().map(|t| t.tok).collect(), diags.list.len())
    }

    #[test]
    fn newlines_follow_the_continuation_rules() {
        let (t, errors) = toks("let x =\n    1 +\n    2\n\n\nfoo(a,\n b)\n  .bar()");
        assert_eq!(errors, 0);
        let newlines = t.iter().filter(|t| **t == Tok::Newline).count();
        assert_eq!(newlines, 2);
    }

    #[test]
    fn numbers_reject_ambiguous_forms() {
        assert_eq!(toks("1_000 0xFF_A0 42i8 3.14f32 1e9").1, 0);
        for bad in ["01", "1_", "1__0", "42i32x", "0x_1", "1f32"] {
            assert_eq!(toks(bad).1, 1, "{bad}");
        }
        assert!(matches!(toks("0..10").0[..3], [Tok::Int { .. }, Tok::DotDot, Tok::Int { .. }]));
    }

    #[test]
    fn strings_decode_escapes_and_interpolation() {
        let (t, errors) = toks(r#""a\n{x + "}"} {{b}}""#);
        assert_eq!(errors, 0);
        let Tok::Str(parts) = &t[0] else { panic!() };
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], StrPart::Text("a\n".into()));
        assert_eq!(parts[2], StrPart::Text(" {b}".into()));
    }

    #[test]
    fn multiline_strings_dedent() {
        let (t, errors) = toks("\"\"\"\n    one\n\n      two\n    \"\"\"");
        assert_eq!(errors, 0);
        assert_eq!(t[0], Tok::Str(vec![StrPart::Text("one\n\n  two".into())]));
    }

    #[test]
    fn forbidden_sequences_are_lexical_errors() {
        assert_eq!(toks("a--b").1, 1);
        assert_eq!(toks("a ?? b").1, 1);
        assert_eq!(toks("a;").1, 1);
    }
}
