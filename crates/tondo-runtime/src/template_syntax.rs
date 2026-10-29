//! The syntax of `std.template` (standard library 14.31), which the
//! compiler reads to compile a template and the runtime to load one: the
//! actions between `{{` and `}}`, the lines that hold only blocks, the place
//! of HTML where each value goes, and the expressions a loaded template
//! evaluates. The format of an interpolation (language 19.6) is read here
//! too, since a template writes values with it. The compiler includes this
//! file as a module of its own.

use std::ops::Range;

// ---------------------------------------------------------------------------
// Formats
// ---------------------------------------------------------------------------

/// The format of an interpolation, `[[fill]align][sign][#][0][width]
/// [.precision][type]` (language 19.6).
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub fill: char,
    /// `<`, `>` or `^`.
    pub align: Option<char>,
    pub plus: bool,
    pub alternate: bool,
    pub zero: bool,
    pub width: Option<u32>,
    pub precision: Option<u32>,
    /// `x`, `X`, `o`, `b`, `e` or `E`.
    pub kind: Option<char>,
}

/// The format a text after `:` writes, or why it is none.
pub fn spec(text: &str) -> Result<Spec, String> {
    let chars: Vec<char> = text.chars().collect();
    let aligned = |c: Option<&char>| matches!(c, Some('<' | '>' | '^'));
    let mut format = Spec {
        fill: ' ',
        align: None,
        plus: false,
        alternate: false,
        zero: false,
        width: None,
        precision: None,
        kind: None,
    };
    let mut at = 0;
    if aligned(chars.get(1)) {
        (format.fill, format.align, at) = (chars[0], Some(chars[1]), 2);
    } else if aligned(chars.first()) {
        (format.align, at) = (Some(chars[0]), 1);
    }
    let flag = |c: char, at: &mut usize| {
        let found = chars.get(*at) == Some(&c);
        *at += found as usize;
        found
    };
    format.plus = flag('+', &mut at);
    format.alternate = flag('#', &mut at);
    format.zero = flag('0', &mut at);
    let number = |at: &mut usize| -> Option<u32> {
        let digits: String = chars[*at..].iter().take_while(|c| c.is_ascii_digit()).collect();
        *at += digits.len();
        digits.parse().ok()
    };
    format.width = number(&mut at);
    if chars.get(at) == Some(&'.') {
        at += 1;
        format.precision = number(&mut at);
        if format.precision.is_none() {
            at = chars.len() + 1;
        }
    }
    if let Some(&kind @ ('x' | 'X' | 'o' | 'b' | 'e' | 'E')) = chars.get(at) {
        format.kind = Some(kind);
        at += 1;
    }
    if at != chars.len() || text.is_empty() {
        return Err(format!(
            "`{text}` is not a format: write `[[fill]align][sign][#][0][width][.precision][type]`, as `>8`, `08`, `.2` or `#x`"
        ));
    }
    Ok(format)
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Where an action writes, which says how its value is escaped: in the text
/// of HTML, or of a text template; in an attribute's value, with or without
/// quotes; at the start of a URL, in its path or in its query; in a script,
/// as code or inside one of its strings; or in a style. Those that end in
/// `Attribute` are in an attribute's value too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Text,
    Attribute,
    Unquoted,
    Url,
    UrlPath,
    UrlQuery,
    Script,
    ScriptAttribute,
    ScriptString,
    ScriptStringAttribute,
    Style,
    StyleAttribute,
}

/// A part of a template. Ranges are the bytes of its source.
#[derive(Clone, Debug)]
pub enum Node {
    Text(String),
    /// A value, its format and where it is written.
    Write {
        value: Range<usize>,
        format: Option<Range<usize>>,
        place: Place,
    },
    If {
        at: usize,
        branches: Vec<(Range<usize>, Vec<Node>)>,
        otherwise: Option<Vec<Node>>,
    },
    For {
        at: usize,
        pattern: Range<usize>,
        items: Range<usize>,
        body: Vec<Node>,
        otherwise: Option<Vec<Node>>,
    },
    With {
        at: usize,
        name: Range<usize>,
        value: Range<usize>,
        body: Vec<Node>,
        otherwise: Option<Vec<Node>>,
    },
    Let {
        name: Range<usize>,
        value: Range<usize>,
    },
    Block {
        at: usize,
        name: String,
        body: Vec<Node>,
    },
    Include {
        at: usize,
        name: String,
        data: Option<Range<usize>>,
    },
    Break,
    Continue,
}

/// A template read: the one it extends, if any, and where it names it, and
/// its parts.
#[derive(Clone, Debug)]
pub struct Template {
    pub extends: Option<(String, usize)>,
    pub nodes: Vec<Node>,
}

/// Why a template or an expression does not read, and where, in bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub at: usize,
    pub message: String,
}

fn error<T>(at: usize, message: impl Into<String>) -> Result<T, Error> {
    Err(Error { at, message: message.into() })
}

/// Reads a template; `html` places its values in HTML.
pub fn parse(source: &str, html: bool) -> Result<Template, Error> {
    let mut pieces = scan(source)?;
    // A file's last line break is not the template's.
    if let Some(Piece::Text(last)) = pieces.last_mut() {
        let end = last.strip_suffix('\n').map_or(last.len(), |rest| rest.strip_suffix('\r').unwrap_or(rest).len());
        last.truncate(end);
    }
    standalone(source, &mut pieces);
    let pieces = trimmed(merged(pieces));
    let mut reader = Reader { source, pieces: pieces.into_iter(), loops: 0, depth: 0, extends: None, seen: false };
    let (mut nodes, stop) = reader.nodes()?;
    match stop {
        Stop::Eof => {}
        Stop::End(at) => return error(at, "this `{{ end }}` closes nothing"),
        Stop::Else(at) | Stop::ElseIf(_, at) => {
            return error(at, "this `{{ else }}` is in no `if`, `for` or `with`");
        }
    }
    if let Some((_, at)) = &reader.extends {
        nodes.retain(|node| !matches!(node, Node::Text(text) if text.trim().is_empty()));
        if nodes.iter().any(|node| !matches!(node, Node::Block { .. })) {
            return error(
                *at,
                "a template that extends another holds only blocks: put what it writes in a `{{ block \"name\" }}`",
            );
        }
    }
    if html {
        let mut state = Html::Text;
        places(&mut nodes, &mut state)?;
        if state != Html::Text {
            return error(source.len(), "the template ends inside a tag or a comment of HTML: close it");
        }
    }
    Ok(Template { extends: reader.extends, nodes })
}

/// Reads a template as a `template.Set` loads it, with each of its
/// expressions, which a loaded template evaluates.
pub fn load(source: &str, html: bool) -> Result<Template, Error> {
    let template = parse(source, html)?;
    fn walk(source: &str, nodes: &[Node]) -> Result<(), Error> {
        for node in nodes {
            match node {
                Node::Text(_) | Node::Break | Node::Continue => {}
                Node::Write { value, .. } | Node::Let { value, .. } => {
                    expression(source, value.clone())?;
                }
                Node::If { branches, otherwise, .. } => {
                    for (condition, body) in branches {
                        expression(source, condition.clone())?;
                        walk(source, body)?;
                    }
                    walk(source, otherwise.as_deref().unwrap_or_default())?;
                }
                Node::For { pattern: names, items, body, otherwise, .. } => {
                    pattern(source, names.clone())?;
                    expression(source, items.clone())?;
                    walk(source, body)?;
                    walk(source, otherwise.as_deref().unwrap_or_default())?;
                }
                Node::With { value, body, otherwise, .. } => {
                    expression(source, value.clone())?;
                    walk(source, body)?;
                    walk(source, otherwise.as_deref().unwrap_or_default())?;
                }
                Node::Block { body, .. } => walk(source, body)?,
                Node::Include { data, .. } => {
                    if let Some(data) = data {
                        expression(source, data.clone())?;
                    }
                }
            }
        }
        Ok(())
    }
    walk(source, &template.nodes)?;
    Ok(template)
}

/// The line and column, from one, of a byte offset of a text.
pub fn line_column(source: &str, at: usize) -> (usize, usize) {
    let before = &source[..at.min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}

/// The names of the templates a source extends or includes, as written,
/// without reading the rest: what a build reads with it.
pub fn references(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(pieces) = scan(source) else { return found };
    for piece in pieces {
        let Piece::Action(action) = piece else { continue };
        let text = &source[action.content];
        for word in ["extends ", "include "] {
            if let Some(rest) = text.strip_prefix(word)
                && let Some(name) = rest.trim().strip_prefix('"').and_then(|rest| rest.split('"').next())
            {
                found.push(name.to_string());
            }
        }
    }
    found
}

enum Piece {
    Text(String),
    Action(Action),
}

/// An action: where its `{{` is, its text without the spaces around, and
/// whether it trims the space before and after it.
struct Action {
    at: usize,
    content: Range<usize>,
    before: bool,
    after: bool,
}

/// The texts and actions of a source.
fn scan(source: &str) -> Result<Vec<Piece>, Error> {
    let mut pieces = Vec::new();
    let mut rest = 0;
    while let Some(found) = source[rest..].find("{{") {
        let open = rest + found;
        pieces.push(Piece::Text(source[rest..open].to_string()));
        let mut start = open + 2;
        let mut chars = source[start..].chars();
        let before = chars.next() == Some('-') && chars.next().is_some_and(char::is_whitespace);
        start += before as usize;
        let Some(close) = closing(source, start) else {
            return error(open, "`{{` starts an action that does not end: close it with `}}`");
        };
        let mut end = close;
        let after = end > start + 1 && source[..end].ends_with('-') && source[..end - 1].ends_with(char::is_whitespace);
        end -= after as usize;
        let text = &source[start..end];
        let first = start + (text.len() - text.trim_start().len());
        let last = (start + text.trim_end().len()).max(first);
        pieces.push(Piece::Action(Action { at: open, content: first..last, before, after }));
        rest = close + 2;
    }
    pieces.push(Piece::Text(source[rest..].to_string()));
    Ok(pieces)
}

/// Where the `}}` of an action that starts at `from` is: outside its
/// strings and characters, or anywhere in a comment.
fn closing(source: &str, from: usize) -> Option<usize> {
    if source[from..].trim_start().starts_with("//") {
        return source[from..].find("}}").map(|at| from + at);
    }
    let bytes = source.as_bytes();
    let mut quote = None;
    let mut at = from;
    while at < bytes.len() {
        match (quote, bytes[at]) {
            (Some(_), b'\\') => at += 1,
            (Some(open), byte) if byte == open => quote = None,
            (Some(_), _) => {}
            (None, byte @ (b'"' | b'\'')) => quote = Some(byte),
            (None, b'}') if bytes.get(at + 1) == Some(&b'}') => return Some(at),
            (None, _) => {}
        }
        at += 1;
    }
    None
}

/// The first word of an action, and where the rest of it is.
fn word(source: &str, range: Range<usize>) -> (&str, Range<usize>) {
    let text = &source[range.clone()];
    let length = text.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(text.len());
    let rest = &text[length..];
    let start = range.start + length + (rest.len() - rest.trim_start().len());
    (&text[..length], start.min(range.end)..range.end)
}

/// Whether an action writes nothing: a block, a binding or a comment.
fn silent(source: &str, action: &Action) -> bool {
    let text = &source[action.content.clone()];
    text.starts_with("//")
        || matches!(
            word(source, action.content.clone()).0,
            "if" | "else" | "end" | "for" | "with" | "let" | "block" | "extends" | "break" | "continue"
        )
}

/// Empties the lines whose actions all write nothing and whose text is
/// only space, with their line breaks, so that blocks leave no lines.
fn standalone(source: &str, pieces: &mut Vec<Piece>) {
    let mut lines: Vec<Piece> = Vec::new();
    for piece in pieces.drain(..) {
        let Piece::Text(text) = piece else {
            lines.push(piece);
            continue;
        };
        let mut rest = text.as_str();
        while let Some(at) = rest.find('\n') {
            lines.push(Piece::Text(rest[..=at].to_string()));
            rest = &rest[at + 1..];
        }
        lines.push(Piece::Text(rest.to_string()));
    }
    let mut start = 0;
    for index in 0..lines.len() {
        let last = index + 1 == lines.len();
        if !last && !matches!(&lines[index], Piece::Text(text) if text.ends_with('\n')) {
            continue;
        }
        let line = &lines[start..=index];
        let mut actions = line.iter().filter_map(|piece| match piece {
            Piece::Action(action) => Some(action),
            Piece::Text(_) => None,
        });
        let blank = line.iter().all(|piece| !matches!(piece, Piece::Text(text) if !text.trim().is_empty()));
        let mut any = false;
        let quiet = actions.all(|action| {
            any = true;
            silent(source, action)
        });
        if blank && any && quiet {
            for piece in &mut lines[start..=index] {
                if let Piece::Text(text) = piece {
                    text.clear();
                }
            }
        }
        start = index + 1;
    }
    *pieces = lines;
}

/// The pieces with each run of texts as one.
fn merged(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for piece in pieces {
        match (out.last_mut(), piece) {
            (Some(Piece::Text(last)), Piece::Text(text)) => last.push_str(&text),
            (_, piece) => out.push(piece),
        }
    }
    out
}

/// The texts without the space that `{{-` and `-}}` trim.
fn trimmed(mut pieces: Vec<Piece>) -> Vec<Piece> {
    for index in 0..pieces.len() {
        let Piece::Action(action) = &pieces[index] else { continue };
        let (before, after) = (action.before, action.after);
        if before && let Some(Piece::Text(text)) = index.checked_sub(1).and_then(|i| pieces.get_mut(i)) {
            text.truncate(text.trim_end().len());
        }
        if after && let Some(Piece::Text(text)) = pieces.get_mut(index + 1) {
            *text = text.trim_start().to_string();
        }
    }
    pieces
}

/// What ends a list of parts.
enum Stop {
    End(usize),
    Else(usize),
    ElseIf(Range<usize>, usize),
    Eof,
}

/// Builds the parts of a template from its pieces.
struct Reader<'a> {
    source: &'a str,
    pieces: std::vec::IntoIter<Piece>,
    /// How many `for`s hold what is read, for `break` and `continue`.
    loops: usize,
    /// How many blocks of any kind hold it.
    depth: usize,
    extends: Option<(String, usize)>,
    /// Whether something but space came already, which `extends` must not
    /// follow.
    seen: bool,
}

impl Reader<'_> {
    fn nodes(&mut self) -> Result<(Vec<Node>, Stop), Error> {
        let mut nodes = Vec::new();
        while let Some(piece) = self.pieces.next() {
            let action = match piece {
                Piece::Text(text) => {
                    self.seen |= !text.trim().is_empty();
                    if !text.is_empty() {
                        nodes.push(Node::Text(text));
                    }
                    continue;
                }
                Piece::Action(action) => action,
            };
            let (at, content) = (action.at, action.content.clone());
            let text = &self.source[content.clone()];
            if text.starts_with("//") {
                continue;
            }
            if text.is_empty() {
                return error(at, "an action is empty: write a value, as `{{ name }}`, or remove it");
            }
            let (first, rest) = word(self.source, content.clone());
            let empty = rest.is_empty();
            let alone = |name: &str| match empty {
                true => Ok(()),
                false => error(rest.start, format!("`{name}` takes nothing more: write `{{{{ {name} }}}}`")),
            };
            let extends = first == "extends";
            match first {
                "end" => {
                    alone("end")?;
                    return Ok((nodes, Stop::End(at)));
                }
                "else" if empty => return Ok((nodes, Stop::Else(at))),
                "else" => match word(self.source, rest.clone()) {
                    ("if", condition) if !condition.is_empty() => return Ok((nodes, Stop::ElseIf(condition, at))),
                    _ => return error(rest.start, "write `{{ else }}` or `{{ else if condition }}`"),
                },
                "if" if !empty => nodes.push(self.conditional(rest, at)?),
                "for" if !empty => nodes.push(self.repeated(rest, at)?),
                "with" if !empty => {
                    let (name, value) = self.binding(rest, "with")?;
                    let (body, otherwise) = self.body(at, "with", true)?;
                    nodes.push(Node::With { at, name, value, body, otherwise });
                }
                "let" if !empty => {
                    let (name, value) = self.binding(rest, "let")?;
                    nodes.push(Node::Let { name, value });
                }
                "block" => {
                    let (name, _) = self.named(rest, "block")?;
                    let (body, _) = self.body(at, "block", false)?;
                    nodes.push(Node::Block { at, name, body });
                }
                "extends" => {
                    let (name, more) = self.named(rest, "extends")?;
                    if more.is_some() || self.seen || self.depth > 0 {
                        return error(at, "`extends` is the first action of a template, and alone");
                    }
                    self.extends = Some((name, at));
                }
                "include" => {
                    let (name, data) = self.named(rest, "include")?;
                    nodes.push(Node::Include { at, name, data });
                }
                "break" | "continue" => {
                    alone(first)?;
                    if self.loops == 0 {
                        return error(at, format!("`{first}` goes in a `for`"));
                    }
                    nodes.push(if first == "break" { Node::Break } else { Node::Continue });
                }
                "if" | "for" | "with" | "let" => {
                    let example = match first {
                        "if" => "{{ if condition }}",
                        "for" => "{{ for item in items }}",
                        "with" => "{{ with value = optional }}",
                        _ => "{{ let name = value }}",
                    };
                    return error(at, format!("`{first}` needs more: write `{example}`"));
                }
                _ => nodes.push(write(self.source, content)?),
            }
            self.seen |= !extends;
        }
        Ok((nodes, Stop::Eof))
    }

    fn conditional(&mut self, condition: Range<usize>, at: usize) -> Result<Node, Error> {
        let mut branches = Vec::new();
        let mut condition = condition;
        self.depth += 1;
        loop {
            let (body, stop) = self.nodes()?;
            branches.push((condition, body));
            match stop {
                Stop::End(_) => {
                    self.depth -= 1;
                    return Ok(Node::If { at, branches, otherwise: None });
                }
                Stop::ElseIf(next, _) => condition = next,
                Stop::Else(_) => {
                    let (otherwise, stop) = self.nodes()?;
                    self.depth -= 1;
                    return match stop {
                        Stop::End(_) => Ok(Node::If { at, branches, otherwise: Some(otherwise) }),
                        _ => unclosed(at, "if"),
                    };
                }
                Stop::Eof => return unclosed(at, "if"),
            }
        }
    }

    fn repeated(&mut self, header: Range<usize>, at: usize) -> Result<Node, Error> {
        let Some((pattern, items)) = split(self.source, header.clone(), Split::In)
            .filter(|(pattern, items)| !pattern.is_empty() && !items.is_empty())
        else {
            return error(header.start, "write `{{ for item in items }}`");
        };
        self.loops += 1;
        let parts = self.body(at, "for", true);
        self.loops -= 1;
        let (body, otherwise) = parts?;
        Ok(Node::For { at, pattern, items, body, otherwise })
    }

    /// The parts until `{{ end }}`, and those after `{{ else }}` when it
    /// may have one.
    fn body(&mut self, at: usize, name: &str, otherwise: bool) -> Result<(Vec<Node>, Option<Vec<Node>>), Error> {
        self.depth += 1;
        let (body, stop) = self.nodes()?;
        let result = match stop {
            Stop::End(_) => Ok((body, None)),
            Stop::Else(_) if otherwise => {
                let loops = std::mem::take(&mut self.loops);
                let (rest, stop) = self.nodes()?;
                self.loops = loops;
                match stop {
                    Stop::End(_) => Ok((body, Some(rest))),
                    _ => unclosed(at, name),
                }
            }
            Stop::Else(at) | Stop::ElseIf(_, at) => error(at, format!("`else` does not go in a `{name}`")),
            Stop::Eof => unclosed(at, name),
        };
        self.depth -= 1;
        result
    }

    /// `name = value` of `with` and `let`.
    fn binding(&self, rest: Range<usize>, what: &str) -> Result<(Range<usize>, Range<usize>), Error> {
        let example = if what == "with" { "{{ with value = optional }}" } else { "{{ let name = value }}" };
        match split(self.source, rest.clone(), Split::Assign) {
            Some((name, value)) if is_name(&self.source[name.clone()]) && !value.is_empty() => Ok((name, value)),
            _ => error(rest.start, format!("write `{example}`")),
        }
    }

    /// The quoted name of a block or a template, and what follows it.
    fn named(&self, rest: Range<usize>, what: &str) -> Result<(String, Option<Range<usize>>), Error> {
        let text = &self.source[rest.clone()];
        let example = match what {
            "block" => "{{ block \"content\" }}",
            "extends" => "{{ extends \"base.html\" }}",
            _ => "{{ include \"card.html\" post }}",
        };
        let Some(name) = text.strip_prefix('"').and_then(|rest| rest.split('"').next()) else {
            return error(rest.start, format!("`{what}` takes a name in quotes: write `{example}`"));
        };
        if name.is_empty() || text.len() < name.len() + 2 {
            return error(rest.start, format!("`{what}` takes a name in quotes: write `{example}`"));
        }
        let after = rest.start + name.len() + 2;
        let more = &self.source[after..rest.end];
        let start = after + (more.len() - more.trim_start().len());
        Ok((name.to_string(), (start < rest.end).then_some(start..rest.end)))
    }
}

fn unclosed<T>(at: usize, name: &str) -> Result<T, Error> {
    error(at, format!("this `{{{{ {name} }}}}` does not end: close it with `{{{{ end }}}}`"))
}

/// A value written, with its format after the first `:` outside brackets
/// and strings, as in an interpolation.
fn write(source: &str, content: Range<usize>) -> Result<Node, Error> {
    match split(source, content.clone(), Split::Colon) {
        Some((value, format)) => {
            if let Err(message) = spec(&source[format.clone()]) {
                return error(format.start, message);
            }
            if value.is_empty() {
                return error(content.start, "a format needs a value before it: write `{{ price:.2 }}`");
            }
            Ok(Node::Write { value, format: Some(format), place: Place::Text })
        }
        None => Ok(Node::Write { value: content, format: None, place: Place::Text }),
    }
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
}

#[derive(Clone, Copy, PartialEq)]
enum Split {
    /// The word `in` of a `for`.
    In,
    /// The `=` of a binding.
    Assign,
    /// The `:` of a format.
    Colon,
}

/// The two sides of the first separator outside brackets and strings,
/// without the space around them.
fn split(source: &str, range: Range<usize>, separator: Split) -> Option<(Range<usize>, Range<usize>)> {
    let bytes = source.as_bytes();
    let (mut depth, mut quote) = (0usize, None);
    let mut at = range.start;
    while at < range.end {
        let byte = bytes[at];
        match quote {
            Some(_) if byte == b'\\' => at += 1,
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ if depth > 0 => {}
                b':' if separator == Split::Colon => return Some(sides(source, range, at, at + 1)),
                b'=' if separator == Split::Assign
                    && !matches!(bytes.get(at + 1), Some(b'=' | b'>'))
                    && !matches!(bytes[..at].last(), Some(b'=' | b'!' | b'<' | b'>')) =>
                {
                    return Some(sides(source, range, at, at + 1));
                }
                b'i' if separator == Split::In
                    && source[at..range.end].starts_with("in")
                    && at + 2 < range.end
                    && bytes[at + 2].is_ascii_whitespace()
                    && at > range.start
                    && bytes[at - 1].is_ascii_whitespace() =>
                {
                    return Some(sides(source, range, at, at + 2));
                }
                _ => {}
            },
        }
        at += 1;
    }
    None
}

fn sides(source: &str, range: Range<usize>, from: usize, to: usize) -> (Range<usize>, Range<usize>) {
    let trim = |part: Range<usize>| {
        let text = &source[part.clone()];
        let start = part.start + (text.len() - text.trim_start().len());
        start..(part.start + text.trim_end().len()).max(start)
    };
    (trim(range.start..from), trim(to..range.end))
}

// ---------------------------------------------------------------------------
// The places of HTML
// ---------------------------------------------------------------------------

/// Where in HTML its text has reached: in text, in a tag or in an
/// attribute, in a comment, or in the text of a script, a style or an
/// element of raw text.
#[derive(Clone, Debug, PartialEq)]
enum Html {
    Text,
    Raw(String),
    Script(Js),
    Style,
    Comment,
    TagName(String),
    EndTag,
    Tag(String),
    AttributeName(String, String),
    AfterName(String, String),
    BeforeValue(String, String),
    Value { tag: String, attribute: String, quote: Option<char>, started: bool, query: bool, js: Js },
}

/// Where a script's text is: in code or in a string.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Js {
    Code,
    String(char),
}

/// What an attribute's value is, by its name.
#[derive(PartialEq)]
enum Kind {
    Plain,
    Url,
    Script,
    Style,
}

fn kind(attribute: &str) -> Kind {
    match attribute {
        "href" | "src" | "action" | "formaction" | "cite" | "poster" | "background" | "data" | "codebase"
        | "longdesc" | "usemap" | "xlink:href" | "manifest" | "icon" | "ping" => Kind::Url,
        "style" => Kind::Style,
        name if name.starts_with("on") => Kind::Script,
        _ => Kind::Plain,
    }
}

/// The place of each value, and the texts without their HTML comments.
fn places(nodes: &mut [Node], state: &mut Html) -> Result<(), Error> {
    for node in nodes {
        match node {
            Node::Text(text) => *text = advance(state, text),
            Node::Write { value, place, .. } => *place = place_at(state, value.start)?,
            Node::If { at, branches, otherwise } => {
                let start = state.clone();
                let mut end = None;
                for (_, body) in branches.iter_mut() {
                    end = Some(branch(body, &start, end, *at)?);
                }
                let mut empty = Vec::new();
                *state = branch(otherwise.as_mut().unwrap_or(&mut empty), &start, end, *at)?;
            }
            Node::With { at, body, otherwise, .. } => {
                let start = state.clone();
                let end = branch(body, &start, None, *at)?;
                let mut empty = Vec::new();
                *state = branch(otherwise.as_mut().unwrap_or(&mut empty), &start, Some(end), *at)?;
            }
            Node::For { at, body, otherwise, .. } => {
                let start = state.clone();
                branch(body, &start, Some(start.clone()), *at)?;
                if let Some(otherwise) = otherwise {
                    branch(otherwise, &start, Some(start.clone()), *at)?;
                }
            }
            Node::Block { at, body, .. } => {
                if !matches!(state, Html::Text | Html::Raw(_)) {
                    return error(*at, "a `block` goes in the text of the HTML, not in a tag");
                }
                let mut inner = state.clone();
                places(body, &mut inner)?;
                if inner != *state {
                    return error(*at, "this block ends inside a tag or a comment of HTML: close it in the block");
                }
            }
            Node::Include { at, .. } if !matches!(state, Html::Text | Html::Raw(_)) => {
                return error(*at, "an `include` goes in the text of the HTML, not in a tag");
            }
            Node::Include { .. } | Node::Let { .. } | Node::Break | Node::Continue => {}
        }
    }
    Ok(())
}

/// The state after a branch that starts at `start`, which must be that of
/// the other branches.
fn branch(body: &mut [Node], start: &Html, other: Option<Html>, at: usize) -> Result<Html, Error> {
    let mut state = start.clone();
    places(body, &mut state)?;
    match other {
        Some(other) if other != state => {
            error(at, "the branches of this block end in different places of the HTML: each closes what it opens")
        }
        _ => Ok(state),
    }
}

/// Where a value written now goes.
fn place_at(state: &mut Html, at: usize) -> Result<Place, Error> {
    match state {
        Html::Text | Html::Raw(_) => Ok(Place::Text),
        Html::Script(Js::Code) => Ok(Place::Script),
        Html::Script(Js::String(_)) => Ok(Place::ScriptString),
        Html::Style => Ok(Place::Style),
        Html::Comment => error(at, "a value cannot go in an HTML comment, which the template does not write"),
        Html::TagName(_) | Html::EndTag | Html::Tag(_) | Html::AttributeName(..) | Html::AfterName(..) => error(
            at,
            "a value cannot write the name of an element or an attribute: write the name, and the value in quotes, as `title=\"{{ title }}\"`",
        ),
        Html::BeforeValue(tag, attribute) | Html::Value { tag, attribute, quote: None, .. } => {
            if kind(attribute) != Kind::Plain {
                return error(at, format!("write the value of `{attribute}` in quotes: `{attribute}=\"...\"`"));
            }
            *state = Html::Value {
                tag: tag.clone(),
                attribute: attribute.clone(),
                quote: None,
                started: true,
                query: false,
                js: Js::Code,
            };
            Ok(Place::Unquoted)
        }
        Html::Value { attribute, started, query, js, .. } => {
            let place = match kind(attribute) {
                Kind::Plain => Place::Attribute,
                Kind::Url if !*started => Place::Url,
                Kind::Url if *query => Place::UrlQuery,
                Kind::Url => Place::UrlPath,
                Kind::Script if *js == Js::Code => Place::ScriptAttribute,
                Kind::Script => Place::ScriptStringAttribute,
                Kind::Style => Place::StyleAttribute,
            };
            *started = true;
            Ok(place)
        }
    }
}

/// The state after a text, and the text without its HTML comments.
fn advance(state: &mut Html, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let c = rest.chars().next().unwrap_or(' ');
        let starts = |prefix: &str| rest.len() >= prefix.len() && rest[..prefix.len()].eq_ignore_ascii_case(prefix);
        let letter_after = |skip: usize| rest[skip..].chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        let mut keep = true;
        let mut width = c.len_utf8();
        match state {
            Html::Text => {
                if rest.starts_with("<!--") {
                    *state = Html::Comment;
                    keep = false;
                    width = 4;
                } else if rest.starts_with("</") && (letter_after(2) || rest.len() == 2) {
                    *state = Html::EndTag;
                } else if c == '<' && (letter_after(1) || rest.len() == 1) {
                    // A `<` just before an action would let its value name a tag.
                    *state = Html::TagName(String::new());
                }
            }
            Html::Comment => {
                keep = false;
                if rest.starts_with("-->") {
                    *state = Html::Text;
                    width = 3;
                }
            }
            Html::Raw(name) => {
                if starts(&format!("</{name}")) {
                    *state = Html::EndTag;
                }
            }
            Html::Script(js) => match *js {
                Js::Code if starts("</script") => *state = Html::EndTag,
                Js::Code if matches!(c, '"' | '\'' | '`') => *js = Js::String(c),
                Js::String(_) if c == '\\' => width += rest[1..].chars().next().map_or(0, char::len_utf8),
                Js::String(open) if c == open => *js = Js::Code,
                _ => {}
            },
            Html::Style => {
                if starts("</style") {
                    *state = Html::EndTag;
                }
            }
            Html::EndTag => {
                if c == '>' {
                    *state = Html::Text;
                }
            }
            Html::TagName(name) => {
                if c.is_ascii_alphanumeric() || c == '-' || c == ':' {
                    name.push(c.to_ascii_lowercase());
                } else {
                    *state = Html::Tag(std::mem::take(name));
                    continue;
                }
            }
            Html::Tag(tag) => match c {
                '>' => *state = inside(tag),
                '/' if rest.starts_with("/>") => {
                    *state = Html::Text;
                    width = 2;
                }
                c if c.is_whitespace() || c == '/' => {}
                c => *state = Html::AttributeName(std::mem::take(tag), c.to_ascii_lowercase().to_string()),
            },
            Html::AttributeName(tag, attribute) => match c {
                '=' => *state = Html::BeforeValue(std::mem::take(tag), std::mem::take(attribute)),
                '>' => *state = inside(tag),
                c if c.is_whitespace() => *state = Html::AfterName(std::mem::take(tag), std::mem::take(attribute)),
                '/' => *state = Html::Tag(std::mem::take(tag)),
                c => attribute.push(c.to_ascii_lowercase()),
            },
            Html::AfterName(tag, attribute) => match c {
                '=' => *state = Html::BeforeValue(std::mem::take(tag), std::mem::take(attribute)),
                '>' => *state = inside(tag),
                c if c.is_whitespace() => {}
                c => *state = Html::AttributeName(std::mem::take(tag), c.to_ascii_lowercase().to_string()),
            },
            Html::BeforeValue(tag, attribute) => match c {
                c if c.is_whitespace() => {}
                '>' => *state = inside(tag),
                '"' | '\'' => {
                    *state = Html::Value {
                        tag: std::mem::take(tag),
                        attribute: std::mem::take(attribute),
                        quote: Some(c),
                        started: false,
                        query: false,
                        js: Js::Code,
                    }
                }
                _ => {
                    *state = Html::Value {
                        tag: std::mem::take(tag),
                        attribute: std::mem::take(attribute),
                        quote: None,
                        started: true,
                        query: false,
                        js: Js::Code,
                    }
                }
            },
            Html::Value { tag, quote, started, query, js, .. } => match *quote {
                Some(open) if c == open => *state = Html::Tag(std::mem::take(tag)),
                None if c.is_whitespace() => *state = Html::Tag(std::mem::take(tag)),
                None if c == '>' => *state = inside(tag),
                _ => {
                    *started = true;
                    *query |= c == '?' || c == '#';
                    *js = match *js {
                        Js::Code if matches!(c, '"' | '\'' | '`') => Js::String(c),
                        Js::String(open) if c == open => Js::Code,
                        js => js,
                    };
                }
            },
        }
        if keep {
            out.push_str(&rest[..width.min(rest.len())]);
        }
        at += width.min(rest.len());
    }
    out
}

/// Where the text after a start tag is.
fn inside(tag: &str) -> Html {
    match tag {
        "script" => Html::Script(Js::Code),
        "style" => Html::Style,
        "textarea" | "title" => Html::Raw(tag.to_string()),
        _ => Html::Text,
    }
}

// ---------------------------------------------------------------------------
// Expressions of loaded templates
// ---------------------------------------------------------------------------

/// An operation of the code of an expression, which a loaded template
/// evaluates with a stack of values.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    /// A variable, a field of the data, or the data itself as `self`.
    Name(String),
    Field(String),
    Index,
    /// A function, by its name, with its count of arguments.
    Call(String, usize),
    Method(String, usize),
    Not,
    Negate,
    Binary(&'static str),
    Range(bool),
    /// `and` and `or`: when the value on the stack decides, this many
    /// operations are skipped, and it stays.
    And(usize),
    Or(usize),
}

/// The code of an expression, with where each operation is, in bytes of
/// the template.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Code {
    pub ops: Vec<Op>,
    pub at: Vec<usize>,
}

/// The code of the expression in `range` of a template.
pub fn expression(source: &str, range: Range<usize>) -> Result<Code, Error> {
    let tokens = tokens(source, range.clone())?;
    let mut parser = Expression { tokens, next: 0, code: Code::default(), end: range.end };
    parser.or()?;
    if let Some((_, at)) = parser.tokens.get(parser.next) {
        return error(*at, "expected the end of the expression");
    }
    Ok(parser.code)
}

/// The names a `for` binds: one, or several in parentheses.
pub fn pattern(source: &str, range: Range<usize>) -> Result<Vec<String>, Error> {
    let text = source[range.clone()].trim();
    let inner = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')).unwrap_or(text);
    let names: Vec<String> = inner.split(',').map(|n| n.trim().to_string()).collect();
    match names.iter().all(|n| is_name(n)) {
        true => Ok(names),
        false => error(range.start, "a `for` binds a name, or names in parentheses: `item` or `(key, value)`"),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Int(i64),
    Float(f64),
    Text(String),
    Name(String),
    Symbol(&'static str),
}

const SYMBOLS: [&str; 20] =
    ["..=", "..", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", ".", ",", "(", ")", "[", "]", "!"];

fn tokens(source: &str, range: Range<usize>) -> Result<Vec<(Token, usize)>, Error> {
    let mut out = Vec::new();
    let mut at = range.start;
    while at < range.end {
        let rest = &source[at..range.end];
        let c = rest.chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            at += c.len_utf8();
            continue;
        }
        if c.is_ascii_digit() {
            let length =
                rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')).unwrap_or(rest.len());
            let mut digits = &rest[..length];
            // `0..3` is a range, not a float.
            if let Some(dots) = digits.find("..") {
                digits = &digits[..dots];
            }
            let clean = digits.replace('_', "");
            let token = if clean.contains(['.', 'e', 'E']) && !clean.starts_with("0x") {
                clean.parse().map(Token::Float).ok()
            } else {
                clean.parse().map(Token::Int).ok()
            };
            let Some(token) = token else { return error(at, format!("`{digits}` is not a number")) };
            out.push((token, at));
            at += digits.len();
            continue;
        }
        if c == '"' {
            let mut text = String::new();
            let mut chars = rest[1..].char_indices();
            let mut end = None;
            while let Some((offset, c)) = chars.next() {
                match c {
                    '"' => {
                        end = Some(offset + 2);
                        break;
                    }
                    '\\' => match chars.next().map(|(_, c)| c) {
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some('r') => text.push('\r'),
                        Some(c @ ('"' | '\\' | '\'')) => text.push(c),
                        _ => return error(at + offset + 1, "write `\\n`, `\\t`, `\\r`, `\\\"` or `\\\\`"),
                    },
                    c => text.push(c),
                }
            }
            let Some(end) = end else { return error(at, "the string does not end: close it with `\"`") };
            out.push((Token::Text(text), at));
            at += end;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let length = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
            out.push((Token::Name(rest[..length].to_string()), at));
            at += length;
            continue;
        }
        let Some(symbol) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) else {
            return error(at, format!("`{c}` does not go in an expression of a template"));
        };
        out.push((Token::Symbol(symbol), at));
        at += symbol.len();
    }
    Ok(out)
}

struct Expression {
    tokens: Vec<(Token, usize)>,
    next: usize,
    code: Code,
    end: usize,
}

impl Expression {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.next).map(|(token, _)| token)
    }

    fn at(&self) -> usize {
        self.tokens.get(self.next).map_or(self.end, |(_, at)| *at)
    }

    fn eat(&mut self, symbol: &str) -> bool {
        let found = matches!(self.peek(), Some(Token::Symbol(s)) if *s == symbol);
        self.next += found as usize;
        found
    }

    fn keyword(&mut self, word: &str) -> bool {
        let found = matches!(self.peek(), Some(Token::Name(name)) if name == word);
        self.next += found as usize;
        found
    }

    fn emit(&mut self, op: Op, at: usize) {
        self.code.ops.push(op);
        self.code.at.push(at);
    }

    fn or(&mut self) -> Result<(), Error> {
        self.and()?;
        while matches!(self.peek(), Some(Token::Name(name)) if name == "or") {
            let at = self.at();
            self.next += 1;
            let jump = self.code.ops.len();
            self.emit(Op::Or(0), at);
            self.and()?;
            self.code.ops[jump] = Op::Or(self.code.ops.len() - jump - 1);
        }
        Ok(())
    }

    fn and(&mut self) -> Result<(), Error> {
        self.not()?;
        while matches!(self.peek(), Some(Token::Name(name)) if name == "and") {
            let at = self.at();
            self.next += 1;
            let jump = self.code.ops.len();
            self.emit(Op::And(0), at);
            self.not()?;
            self.code.ops[jump] = Op::And(self.code.ops.len() - jump - 1);
        }
        Ok(())
    }

    fn not(&mut self) -> Result<(), Error> {
        let at = self.at();
        if self.keyword("not") {
            self.not()?;
            self.emit(Op::Not, at);
            return Ok(());
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<(), Error> {
        self.range()?;
        let at = self.at();
        let operator = match self.peek() {
            Some(Token::Symbol(s @ ("==" | "!=" | "<" | "<=" | ">" | ">="))) => *s,
            Some(Token::Name(name)) if name == "in" => "in",
            _ => return Ok(()),
        };
        self.next += 1;
        self.range()?;
        self.emit(Op::Binary(operator), at);
        Ok(())
    }

    fn range(&mut self) -> Result<(), Error> {
        self.additive()?;
        let at = self.at();
        let inclusive = if self.eat("..=") {
            true
        } else if self.eat("..") {
            false
        } else {
            return Ok(());
        };
        self.additive()?;
        self.emit(Op::Range(inclusive), at);
        Ok(())
    }

    fn additive(&mut self) -> Result<(), Error> {
        self.multiplicative()?;
        loop {
            let at = self.at();
            let operator = if self.eat("+") {
                "+"
            } else if self.eat("-") {
                "-"
            } else {
                return Ok(());
            };
            self.multiplicative()?;
            self.emit(Op::Binary(operator), at);
        }
    }

    fn multiplicative(&mut self) -> Result<(), Error> {
        self.unary()?;
        loop {
            let at = self.at();
            let operator = if self.eat("*") {
                "*"
            } else if self.eat("/") {
                "/"
            } else if self.eat("%") {
                "%"
            } else {
                return Ok(());
            };
            self.unary()?;
            self.emit(Op::Binary(operator), at);
        }
    }

    fn unary(&mut self) -> Result<(), Error> {
        let at = self.at();
        if self.eat("-") {
            self.unary()?;
            self.emit(Op::Negate, at);
            return Ok(());
        }
        if self.eat("!") {
            return error(at, "write `not` for `!`");
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<(), Error> {
        self.primary()?;
        loop {
            let at = self.at();
            if self.eat(".") {
                let Some(Token::Name(name)) = self.peek().cloned() else {
                    return error(self.at(), "expected the name of a field or a method after `.`");
                };
                self.next += 1;
                if self.eat("(") {
                    let count = self.arguments()?;
                    self.emit(Op::Method(name, count), at);
                } else {
                    self.emit(Op::Field(name), at);
                }
            } else if self.eat("[") {
                self.or()?;
                if !self.eat("]") {
                    return error(self.at(), "expected `]`");
                }
                self.emit(Op::Index, at);
            } else {
                return Ok(());
            }
        }
    }

    /// The arguments after `(`, up to `)`, and how many.
    fn arguments(&mut self) -> Result<usize, Error> {
        let mut count = 0;
        if self.eat(")") {
            return Ok(0);
        }
        loop {
            self.or()?;
            count += 1;
            if self.eat(")") {
                return Ok(count);
            }
            if !self.eat(",") {
                return error(self.at(), "expected `,` or `)`");
            }
        }
    }

    fn primary(&mut self) -> Result<(), Error> {
        let at = self.at();
        let Some(token) = self.peek().cloned() else { return error(at, "expected a value") };
        self.next += 1;
        match token {
            Token::Int(value) => self.emit(Op::Int(value), at),
            Token::Float(value) => self.emit(Op::Float(value), at),
            Token::Text(text) => self.emit(Op::Text(text), at),
            Token::Name(name) if name == "true" || name == "false" => self.emit(Op::Bool(name == "true"), at),
            Token::Name(name) if matches!(name.as_str(), "and" | "or" | "not" | "in") => {
                return error(at, format!("expected a value before `{name}`"));
            }
            Token::Name(name) => {
                if self.eat("(") {
                    let count = self.arguments()?;
                    self.emit(Op::Call(name, count), at);
                } else {
                    self.emit(Op::Name(name), at);
                }
            }
            Token::Symbol("(") => {
                self.or()?;
                if !self.eat(")") {
                    return error(self.at(), "expected `)`");
                }
            }
            Token::Symbol(symbol) => return error(at, format!("expected a value, found `{symbol}`")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(template: &Template) -> String {
        let mut out = String::new();
        for node in &template.nodes {
            match node {
                Node::Text(text) => out.push_str(text),
                Node::Write { place, .. } => out.push_str(&format!("[{place:?}]")),
                Node::If { .. } => out.push_str("[if]"),
                Node::For { .. } => out.push_str("[for]"),
                _ => out.push_str("[other]"),
            }
        }
        out
    }

    #[test]
    fn lines_with_blocks_alone_vanish() {
        let source = "<ul>\n  {{ for x in xs }}\n  <li>{{ x }}</li>\n  {{ end }}\n</ul>\n";
        let template = parse(source, true).unwrap();
        assert_eq!(texts(&template), "<ul>\n[for]</ul>");
        let Node::For { body, .. } = &template.nodes[1] else { panic!() };
        assert!(matches!(&body[0], Node::Text(text) if text == "  <li>"));
    }

    #[test]
    fn places_follow_the_html() {
        let source = r#"<a href="{{ url }}?q={{ q }}" title={{ t }} onclick="go({{ x }})">{{ name }}</a><script>let s = "{{ s }}"; let v = {{ v }}</script><style>p { color: {{ c }} }</style><!-- {{ hidden }}"#;
        let failure = parse(source, true).unwrap_err();
        assert!(failure.message.contains("comment"), "{failure:?}");
        let source = &source[..source.find("<!--").unwrap()];
        let template = parse(source, true).unwrap();
        let places: Vec<Place> = template
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Write { place, .. } => Some(*place),
                _ => None,
            })
            .collect();
        use Place::*;
        assert_eq!(places, [Url, UrlQuery, Unquoted, ScriptAttribute, Text, ScriptString, Script, Style]);
        assert!(parse("<{{ tag }}>", true).is_err());
        assert!(parse("<a {{ attribute }}>", true).is_err());
    }

    #[test]
    fn expressions_compile_to_code() {
        let source = "not a.b(1, \"x\")[0] == 2 and c or d";
        let code = expression(source, 0..source.len()).unwrap();
        assert_eq!(code.ops.len(), 13, "{:?}", code.ops);
        assert!(expression("a +", 0..3).is_err());
        assert_eq!(pattern("(k, v)", 0..6).unwrap(), ["k", "v"]);
    }

    #[test]
    fn structure_errors_say_what_to_write() {
        let failure = parse("{{ if x }}a", false).unwrap_err();
        assert!(failure.message.contains("does not end"), "{failure:?}");
        assert!(parse("{{ end }}", false).is_err());
        assert!(parse("{{ break }}", false).is_err());
        let template = parse("{{ extends \"base.html\" }}\n{{ block \"main\" }}x{{ end }}\n", false).unwrap();
        assert_eq!(template.extends.as_ref().map(|(n, _)| n.as_str()), Some("base.html"));
        assert!(parse("{{ extends \"base.html\" }}text", false).is_err());
        assert_eq!(references("{{ include \"a.html\" x }}{{ extends \"b.html\" }}"), ["a.html", "b.html"]);
    }
}
