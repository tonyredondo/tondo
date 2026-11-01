//! `.proto` files as modules (standard library 14.12): each file of a
//! package's sources is the module of its path, `src/api/user.proto` is
//! `app.api.user`, whose types are the records and enums its messages and
//! enums become, annotated for `std.protobuf`. The files of a package are
//! read together, since one names the types of another by their full names;
//! what a file says wrong is `E1911` at its line.

use crate::diag::Diagnostics;
use crate::source::{SourceMap, Span};
use std::collections::HashMap;
use std::fmt::Write;

// ---------------------------------------------------------------------------
// The syntax of a .proto file
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Name(String),
    Int(i128),
    Float(String),
    Text(String),
    Symbol(char),
}

#[derive(Clone, Copy, PartialEq)]
enum Syntax {
    Proto2,
    Proto3,
}

#[derive(Clone, Copy, PartialEq)]
enum Label {
    None,
    Optional,
    Required,
    Repeated,
}

struct Field {
    label: Label,
    /// The type as written: a scalar's name or a message's or enum's.
    ty: String,
    /// A map's key and value types.
    map: Option<(String, String)>,
    name: String,
    number: i128,
    span: Span,
    /// Where its type is written.
    type_span: Span,
    default: Option<Token>,
    packed: Option<bool>,
    json_name: Option<String>,
    /// A group's own message.
    group: Option<Box<Message>>,
    /// Its options but `default` and `json_name`, for its descriptor.
    options: Options,
}

/// Options as written: `deprecated = true`, `(my.option) = 3`.
type Options = Vec<(String, Token)>;

/// What a message or an enum reserves, or the numbers a message leaves to
/// extensions: ranges with both ends, and names.
#[derive(Default)]
struct Reserved {
    ranges: Vec<(i128, i128)>,
    names: Vec<String>,
}

struct OneOf {
    name: String,
    fields: Vec<Field>,
}

struct Message {
    name: String,
    /// Where its name is, which orders it among the types of its message.
    span: Span,
    fields: Vec<Field>,
    oneofs: Vec<OneOf>,
    messages: Vec<Message>,
    enums: Vec<Enum>,
    options: Options,
    reserved: Reserved,
    extension_ranges: Vec<(i128, i128)>,
    /// The fields its `extend` blocks add to other messages, each after the
    /// message it extends.
    extensions: Vec<(String, Field)>,
}

struct Enum {
    name: String,
    span: Span,
    values: Vec<(String, i128, Span)>,
    /// Each value's options.
    value_options: Vec<Options>,
    options: Options,
    reserved: Reserved,
}

pub struct Method {
    pub name: String,
    pub input: String,
    pub output: String,
    pub client_streams: bool,
    pub server_streams: bool,
    pub span: Span,
    options: Options,
}

pub struct Service {
    pub name: String,
    pub methods: Vec<Method>,
    options: Options,
}

struct File {
    syntax: Syntax,
    package: String,
    imports: Vec<(String, Span)>,
    /// The imports marked `public` and `weak`, by their index.
    public: Vec<usize>,
    weak: Vec<usize>,
    messages: Vec<Message>,
    enums: Vec<Enum>,
    services: Vec<Service>,
    options: Options,
    extensions: Vec<(String, Field)>,
}

struct Parser<'a> {
    tokens: Vec<(Token, Span)>,
    next: usize,
    diags: &'a mut Diagnostics,
    end: Span,
}

type Parsed<T> = Result<T, ()>;

fn tokens(text: &str, base: u32, diags: &mut Diagnostics) -> Option<Vec<(Token, Span)>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    let span = |start: usize, end: usize| Span::new(base + start as u32, base + end as u32);
    while at < bytes.len() {
        let c = bytes[at];
        if c.is_ascii_whitespace() {
            at += 1;
        } else if text[at..].starts_with("//") {
            at = text[at..].find('\n').map_or(bytes.len(), |n| at + n);
        } else if text[at..].starts_with("/*") {
            match text[at + 2..].find("*/") {
                Some(n) => at += n + 4,
                None => {
                    diags.error("E1911", span(at, at + 2), "this comment does not end: close it with `*/`");
                    return None;
                }
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            out.push((Token::Name(text[start..at].to_string()), span(start, at)));
        } else if c.is_ascii_digit() || (c == b'.' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit)) {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'.') {
                // An exponent's sign.
                if matches!(bytes[at], b'e' | b'E')
                    && !text[start..at].starts_with("0x")
                    && matches!(bytes.get(at + 1), Some(b'+' | b'-'))
                {
                    at += 1;
                }
                at += 1;
            }
            let written = &text[start..at];
            let parsed = if let Some(hex) = written.strip_prefix("0x").or_else(|| written.strip_prefix("0X")) {
                i128::from_str_radix(hex, 16).ok().map(Token::Int)
            } else if written.contains(['.', 'e', 'E']) {
                Some(Token::Float(written.to_string()))
            } else if written.len() > 1 && written.starts_with('0') {
                i128::from_str_radix(&written[1..], 8).ok().map(Token::Int)
            } else {
                written.parse().ok().map(Token::Int)
            };
            let Some(token) = parsed else {
                diags.error("E1911", span(start, at), format!("`{written}` is not a number"));
                return None;
            };
            out.push((token, span(start, at)));
        } else if c == b'"' || c == b'\'' {
            let start = at;
            at += 1;
            let mut value = String::new();
            loop {
                let Some(&b) = bytes.get(at) else {
                    diags.error("E1911", span(start, at), "this string does not end: close it with its quote");
                    return None;
                };
                if b == c {
                    at += 1;
                    break;
                }
                if b == b'\\' {
                    let escaped = bytes.get(at + 1).copied().unwrap_or(b'\\');
                    at += 2;
                    match escaped {
                        b'n' => value.push('\n'),
                        b't' => value.push('\t'),
                        b'r' => value.push('\r'),
                        b'0'..=b'7' => {
                            let mut code = (escaped - b'0') as u32;
                            for _ in 0..2 {
                                if let Some(d) = bytes.get(at).filter(|d| (b'0'..=b'7').contains(d)) {
                                    code = code * 8 + (d - b'0') as u32;
                                    at += 1;
                                }
                            }
                            value.push(char::from_u32(code).unwrap_or('?'));
                        }
                        other => value.push(other as char),
                    }
                    continue;
                }
                let ch = text[at..].chars().next().unwrap_or(' ');
                value.push(ch);
                at += ch.len_utf8();
            }
            out.push((Token::Text(value), span(start, at)));
        } else {
            out.push((Token::Symbol(c as char), span(at, at + 1)));
            at += 1;
        }
    }
    Some(out)
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.next).map(|(t, _)| t)
    }

    fn span(&self) -> Span {
        self.tokens.get(self.next).map_or(self.end, |(_, s)| *s)
    }

    fn fail<T>(&mut self, message: impl Into<String>) -> Parsed<T> {
        let span = self.span();
        self.diags.error("E1911", span, message);
        Err(())
    }

    fn eat(&mut self, symbol: char) -> bool {
        let found = self.peek() == Some(&Token::Symbol(symbol));
        self.next += found as usize;
        found
    }

    fn expect(&mut self, symbol: char) -> Parsed<()> {
        if self.eat(symbol) { Ok(()) } else { self.fail(format!("expected `{symbol}`")) }
    }

    fn word(&mut self, word: &str) -> bool {
        let found = matches!(self.peek(), Some(Token::Name(name)) if name == word);
        self.next += found as usize;
        found
    }

    fn name(&mut self) -> Parsed<String> {
        match self.peek().cloned() {
            Some(Token::Name(name)) => {
                self.next += 1;
                Ok(name)
            }
            _ => self.fail("expected a name"),
        }
    }

    /// A dotted name, maybe fully qualified: `.google.protobuf.Timestamp`.
    fn full_name(&mut self) -> Parsed<String> {
        let mut text = String::new();
        if self.eat('.') {
            text.push('.');
        }
        text.push_str(&self.name()?);
        while self.eat('.') {
            text.push('.');
            text.push_str(&self.name()?);
        }
        Ok(text)
    }

    fn integer(&mut self) -> Parsed<i128> {
        let negative = self.eat('-');
        match self.peek().cloned() {
            Some(Token::Int(value)) => {
                self.next += 1;
                Ok(if negative { -value } else { value })
            }
            _ => self.fail("expected an integer"),
        }
    }

    fn text(&mut self) -> Parsed<String> {
        let mut value = match self.peek().cloned() {
            Some(Token::Text(text)) => text,
            _ => return self.fail("expected a string"),
        };
        self.next += 1;
        // Strings next to each other are one.
        while let Some(Token::Text(more)) = self.peek().cloned() {
            value.push_str(&more);
            self.next += 1;
        }
        Ok(value)
    }

    /// A constant of an option: a number, a string, a name or `{...}`.
    fn constant(&mut self) -> Parsed<Token> {
        if self.peek() == Some(&Token::Symbol('{')) {
            self.skip_braces()?;
            return Ok(Token::Name(String::new()));
        }
        let negative = self.eat('-');
        if !negative {
            self.eat('+');
        }
        let token = match self.peek().cloned() {
            Some(Token::Int(value)) => Token::Int(if negative { -value } else { value }),
            Some(Token::Float(text)) => Token::Float(if negative { format!("-{text}") } else { text }),
            Some(Token::Text(_)) => return self.text().map(Token::Text),
            Some(Token::Name(name)) if negative => Token::Name(format!("-{name}")),
            Some(Token::Name(_)) => return self.full_name().map(Token::Name),
            _ => return self.fail("expected a constant"),
        };
        self.next += 1;
        Ok(token)
    }

    fn skip_braces(&mut self) -> Parsed<()> {
        self.expect('{')?;
        let mut depth = 1;
        while depth > 0 {
            match self.peek() {
                None => return self.fail("this `{` does not close"),
                Some(Token::Symbol('{')) => depth += 1,
                Some(Token::Symbol('}')) => depth -= 1,
                _ => {}
            }
            self.next += 1;
        }
        Ok(())
    }

    /// Skips to the end of a statement, `;`, or of a block.
    fn skip_statement(&mut self) -> Parsed<()> {
        loop {
            match self.peek() {
                None => return self.fail("expected `;`"),
                Some(Token::Symbol(';')) => {
                    self.next += 1;
                    return Ok(());
                }
                Some(Token::Symbol('{')) => return self.skip_braces(),
                _ => self.next += 1,
            }
        }
    }

    /// `option name = value;`, after `option`.
    fn option(&mut self) -> Parsed<(String, Token)> {
        let name = self.option_name()?;
        self.expect('=')?;
        let value = self.constant()?;
        self.expect(';')?;
        Ok((name, value))
    }

    /// `reserved 2, 9 to 11, 40 to max;`, `reserved "old";` or the numbers
    /// of `extensions`, after the word: each range with both ends.
    fn reserved(&mut self, max: i128) -> Parsed<Reserved> {
        let mut reserved = Reserved::default();
        loop {
            if let Some(Token::Text(_)) = self.peek() {
                reserved.names.push(self.text()?);
            } else {
                let start = self.integer()?;
                let end = if !self.word("to") {
                    start
                } else if self.word("max") {
                    max
                } else {
                    self.integer()?
                };
                reserved.ranges.push((start, end));
            }
            if !self.eat(',') {
                break;
            }
        }
        self.field_options()?;
        self.expect(';')?;
        Ok(reserved)
    }

    /// `extend Name { fields }`, after `extend`: each field with the name
    /// of the message it extends, as written.
    fn extend(&mut self, syntax: Syntax) -> Parsed<Vec<(String, Field)>> {
        let extendee = self.full_name()?;
        let mut fields = Vec::new();
        self.expect('{')?;
        while !self.eat('}') {
            if self.peek().is_none() {
                return self.fail("this `extend` does not close: add `}`");
            }
            if self.eat(';') {
                continue;
            }
            let label = if self.word("optional") {
                Label::Optional
            } else if self.word("required") {
                Label::Required
            } else if self.word("repeated") {
                Label::Repeated
            } else {
                Label::None
            };
            fields.push((extendee.clone(), self.field(label, syntax)?));
        }
        Ok(fields)
    }

    /// The name of an option: `packed`, `(my.option).field`.
    fn option_name(&mut self) -> Parsed<String> {
        let mut text = String::new();
        loop {
            if self.eat('(') {
                text.push('(');
                text.push_str(&self.full_name()?);
                self.expect(')')?;
                text.push(')');
            } else {
                text.push_str(&self.name()?);
            }
            if !self.eat('.') {
                return Ok(text);
            }
            text.push('.');
        }
    }

    /// `[packed = true, default = 5]`, as names and constants.
    fn field_options(&mut self) -> Parsed<Vec<(String, Token)>> {
        let mut options = Vec::new();
        if !self.eat('[') {
            return Ok(options);
        }
        loop {
            let name = self.option_name()?;
            self.expect('=')?;
            options.push((name, self.constant()?));
            if self.eat(']') {
                return Ok(options);
            }
            self.expect(',')?;
        }
    }

    fn file(&mut self) -> Parsed<File> {
        let mut file = File {
            syntax: Syntax::Proto2,
            package: String::new(),
            imports: Vec::new(),
            public: Vec::new(),
            weak: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
            services: Vec::new(),
            options: Vec::new(),
            extensions: Vec::new(),
        };
        while self.peek().is_some() {
            if self.eat(';') {
                continue;
            }
            let span = self.span();
            match self.name()?.as_str() {
                "syntax" => {
                    self.expect('=')?;
                    file.syntax = match self.text()?.as_str() {
                        "proto2" => Syntax::Proto2,
                        "proto3" => Syntax::Proto3,
                        other => {
                            self.diags.error(
                                "E1911",
                                span,
                                format!(
                                    "`{other}` is no syntax of protobuf: write `syntax = \"proto3\";` or `\"proto2\"`"
                                ),
                            );
                            return Err(());
                        }
                    };
                    self.expect(';')?;
                }
                "edition" => {
                    let message = "protobuf editions are not supported: write `syntax = \"proto3\";` or `syntax = \"proto2\";`, which `protoc` can say how to translate";
                    self.diags.error("E1911", span, message);
                    return Err(());
                }
                "package" => {
                    file.package = self.full_name()?;
                    self.expect(';')?;
                }
                "import" => {
                    if self.word("public") {
                        file.public.push(file.imports.len());
                    } else if self.word("weak") {
                        file.weak.push(file.imports.len());
                    }
                    let span = self.span();
                    file.imports.push((self.text()?, span));
                    self.expect(';')?;
                }
                "option" => file.options.push(self.option()?),
                "message" => file.messages.push(self.message(file.syntax)?),
                "enum" => file.enums.push(self.enumeration()?),
                "service" => file.services.push(self.service()?),
                "extend" => {
                    let syntax = file.syntax;
                    file.extensions.extend(self.extend(syntax)?);
                }
                other => {
                    self.diags.error("E1911", span, format!("expected `message`, `enum`, `service`, `import`, `package`, `option` or `syntax`, found `{other}`"));
                    return Err(());
                }
            }
        }
        Ok(file)
    }

    fn message(&mut self, syntax: Syntax) -> Parsed<Message> {
        let span = self.span();
        let name = self.name()?;
        self.message_body(name, span, syntax)
    }

    /// The `{ ... }` of a message of a name, or of a group.
    fn message_body(&mut self, name: String, span: Span, syntax: Syntax) -> Parsed<Message> {
        let mut message = Message {
            name,
            span,
            fields: Vec::new(),
            oneofs: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
            options: Vec::new(),
            reserved: Reserved::default(),
            extension_ranges: Vec::new(),
            extensions: Vec::new(),
        };
        self.expect('{')?;
        while !self.eat('}') {
            if self.peek().is_none() {
                return self.fail("this message does not close: add `}`");
            }
            if self.eat(';') {
                continue;
            }
            let save = self.next;
            match self.name()?.as_str() {
                "message" => message.messages.push(self.message(syntax)?),
                "enum" => message.enums.push(self.enumeration()?),
                "option" => message.options.push(self.option()?),
                "reserved" => {
                    let reserved = self.reserved(MAX_FIELD)?;
                    message.reserved.ranges.extend(reserved.ranges);
                    message.reserved.names.extend(reserved.names);
                }
                "extensions" => message.extension_ranges.extend(self.reserved(MAX_FIELD)?.ranges),
                "extend" => message.extensions.extend(self.extend(syntax)?),
                "oneof" => {
                    let name = self.name()?;
                    let mut oneof = OneOf { name, fields: Vec::new() };
                    self.expect('{')?;
                    while !self.eat('}') {
                        if self.eat(';') {
                            continue;
                        }
                        if self.word("option") {
                            self.skip_statement()?;
                            continue;
                        }
                        oneof.fields.push(self.field(Label::None, syntax)?);
                    }
                    message.oneofs.push(oneof);
                }
                "map" if self.peek() == Some(&Token::Symbol('<')) => {
                    self.expect('<')?;
                    let key = self.full_name()?;
                    self.expect(',')?;
                    let value = self.full_name()?;
                    self.expect('>')?;
                    let span = self.span();
                    let name = self.name()?;
                    self.expect('=')?;
                    let number = self.integer()?;
                    let options = self.field_options()?;
                    self.expect(';')?;
                    let json_name = options.iter().find_map(|(n, v)| match v {
                        Token::Text(text) if n == "json_name" => Some(text.clone()),
                        _ => None,
                    });
                    message.fields.push(Field {
                        label: Label::None,
                        ty: String::new(),
                        map: Some((key, value)),
                        name,
                        number,
                        span,
                        type_span: span,
                        default: None,
                        packed: None,
                        json_name,
                        group: None,
                        options: options
                            .into_iter()
                            .filter(|(name, _)| name != "default" && name != "json_name")
                            .collect(),
                    });
                }
                label @ ("optional" | "required" | "repeated") => {
                    let label = match label {
                        "optional" => Label::Optional,
                        "required" => Label::Required,
                        _ => Label::Repeated,
                    };
                    message.fields.push(self.field(label, syntax)?);
                }
                _ => {
                    self.next = save;
                    message.fields.push(self.field(Label::None, syntax)?);
                }
            }
        }
        Ok(message)
    }

    /// A field after its label: `int32 id = 1 [packed = false];`, or a
    /// group, whose body is a message of its name and whose field is that
    /// name in lowercase, as protoc names it.
    fn field(&mut self, label: Label, syntax: Syntax) -> Parsed<Field> {
        if self.word("group") {
            let span = self.span();
            let name = self.name()?;
            self.expect('=')?;
            let number = self.integer()?;
            let options = self.field_options()?;
            let group = self.message_body(name.clone(), span, syntax)?;
            return Ok(Field {
                label,
                ty: name.clone(),
                map: None,
                name: name.to_lowercase(),
                number,
                span,
                type_span: span,
                default: None,
                packed: None,
                json_name: None,
                group: Some(Box::new(group)),
                options,
            });
        }
        let type_span = self.span();
        let ty = self.full_name()?;
        let span = self.span();
        let name = self.name()?;
        self.expect('=')?;
        let number = self.integer()?;
        let options = self.field_options()?;
        self.expect(';')?;
        let option = |name: &str| options.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        Ok(Field {
            label,
            ty,
            map: None,
            name,
            number,
            span,
            type_span,
            default: option("default"),
            packed: option("packed").map(|v| v == Token::Name("true".into())),
            json_name: match option("json_name") {
                Some(Token::Text(text)) => Some(text),
                _ => None,
            },
            group: None,
            options: options.iter().filter(|(name, _)| name != "default" && name != "json_name").cloned().collect(),
        })
    }

    fn enumeration(&mut self) -> Parsed<Enum> {
        let span = self.span();
        let name = self.name()?;
        let mut values = Vec::new();
        let mut value_options = Vec::new();
        let mut options = Vec::new();
        let mut reserved = Reserved::default();
        self.expect('{')?;
        while !self.eat('}') {
            if self.peek().is_none() {
                return self.fail("this enum does not close: add `}`");
            }
            if self.eat(';') {
                continue;
            }
            if self.word("option") {
                options.push(self.option()?);
                continue;
            }
            if self.word("reserved") {
                let more = self.reserved(i128::from(i32::MAX))?;
                reserved.ranges.extend(more.ranges);
                reserved.names.extend(more.names);
                continue;
            }
            let span = self.span();
            let value = self.name()?;
            self.expect('=')?;
            let number = self.integer()?;
            value_options.push(self.field_options()?);
            self.expect(';')?;
            values.push((value, number, span));
        }
        Ok(Enum { name, span, values, value_options, options, reserved })
    }

    fn service(&mut self) -> Parsed<Service> {
        let name = self.name()?;
        let mut service = Service { name, methods: Vec::new(), options: Vec::new() };
        self.expect('{')?;
        while !self.eat('}') {
            if self.peek().is_none() {
                return self.fail("this service does not close: add `}`");
            }
            if self.eat(';') {
                continue;
            }
            if self.word("option") {
                service.options.push(self.option()?);
                continue;
            }
            if !self.word("rpc") {
                return self.fail("expected `rpc`");
            }
            let span = self.span();
            let name = self.name()?;
            self.expect('(')?;
            let client_streams = self.word("stream");
            let input = self.full_name()?;
            self.expect(')')?;
            if !self.word("returns") {
                return self.fail("expected `returns`");
            }
            self.expect('(')?;
            let server_streams = self.word("stream");
            let output = self.full_name()?;
            self.expect(')')?;
            let mut options = Vec::new();
            if self.eat('{') {
                while !self.eat('}') {
                    if self.eat(';') {
                        continue;
                    }
                    if !self.word("option") {
                        return self.fail("expected `option` or `}`");
                    }
                    options.push(self.option()?);
                }
            } else {
                self.expect(';')?;
            }
            service.methods.push(Method { name, input, output, client_streams, server_streams, span, options });
        }
        Ok(service)
    }
}

/// The largest number of a message's field, which `max` names.
const MAX_FIELD: i128 = (1 << 29) - 1;

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// `user_id` as `userId`; a keyword of Tondo gets `_` after it.
fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for (index, c) in name.chars().enumerate() {
        if c == '_' {
            upper = index > 0;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else if out.is_empty() {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    unreserved(out)
}

/// `STATUS_ACTIVE` or `active_now` as `ActiveNow`-style Pascal case.
fn pascal(name: &str) -> String {
    let all_upper = name.chars().all(|c| !c.is_lowercase());
    let mut out = String::new();
    for part in name.split('_').filter(|p| !p.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            if all_upper {
                out.extend(chars.flat_map(char::to_lowercase));
            } else {
                out.extend(chars);
            }
        }
    }
    out
}

fn unreserved(name: String) -> String {
    if crate::lexer::Kw::lookup(&name).is_some() { format!("{name}_") } else { name }
}

/// A type's name that the prelude does not have already.
fn type_name(name: String, kind: &str) -> String {
    if crate::check::PRELUDE_TYPES.contains(&name.as_str()) { format!("{name}{kind}") } else { name }
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// What a full name of protobuf is in Tondo.
#[derive(Clone)]
struct Symbol {
    /// The module it is in, and its type there.
    module: Vec<String>,
    name: String,
    kind: SymbolKind,
}

#[derive(Clone)]
enum SymbolKind {
    Message,
    /// An enum, with the variant of its first value, its default.
    Enum(String),
}

/// A `.proto` file of a package: its module and its text among the sources.
pub struct ProtoSource {
    pub module: Vec<String>,
    pub source: usize,
    /// Its path from the source root, as other files import it.
    pub relative: String,
}

/// The Tondo source of each file, by module, and the services each has, for
/// gRPC; files that do not read give nothing.
pub fn translate(files: &[ProtoSource], sources: &SourceMap, diags: &mut Diagnostics) -> Vec<(Vec<String>, String)> {
    let mut parsed: Vec<Option<File>> = Vec::new();
    for file in files {
        let source = sources.file(file.source);
        let end = Span::new(source.base + source.text.len() as u32, source.base + source.text.len() as u32);
        let Some(tokens) = tokens(&source.text, source.base, diags) else {
            parsed.push(None);
            continue;
        };
        let mut parser = Parser { tokens, next: 0, diags, end };
        parsed.push(parser.file().ok());
    }
    // Every message and enum of the package, by full name.
    let mut symbols: HashMap<String, Symbol> = HashMap::new();
    for (file, parsed) in files.iter().zip(&parsed) {
        let Some(parsed) = parsed else { continue };
        let prefix = if parsed.package.is_empty() { String::new() } else { format!("{}.", parsed.package) };
        for message in &parsed.messages {
            collect_message(message, &prefix, "", &file.module, &mut symbols);
        }
        for e in &parsed.enums {
            collect_enum(e, &prefix, "", &file.module, &mut symbols);
        }
    }
    // What each name is, for the descriptors: the package's and those of
    // the files of `google.protobuf`.
    let mut kinds: HashMap<String, bool> =
        symbols.iter().map(|(name, symbol)| (name.clone(), matches!(symbol.kind, SymbolKind::Enum(_)))).collect();
    let mut well_known: Vec<(&str, File)> = Vec::new();
    for (name, body) in WELL_KNOWN_FILES {
        let text = format!("syntax = \"proto3\"; package google.protobuf; {body}");
        let mut quiet = Diagnostics::default();
        let Some(tokens) = tokens(&text, 0, &mut quiet) else { continue };
        let mut parser = Parser { tokens, next: 0, diags: &mut quiet, end: Span::new(0, 0) };
        let Ok(file) = parser.file() else { continue };
        for message in &file.messages {
            kinds.insert(format!("google.protobuf.{}", message.name), false);
        }
        for e in &file.enums {
            kinds.insert(format!("google.protobuf.{}", e.name), true);
        }
        well_known.push((name, file));
    }
    let well_known: Vec<(&str, Described)> =
        well_known.iter().map(|(name, file)| (*name, Describer::file(&kinds, file, name))).collect();
    let mut out = Vec::new();
    for (file, parsed) in files.iter().zip(&parsed) {
        let Some(parsed) = parsed else { continue };
        let mut writer = Translator {
            file: parsed,
            module: &file.module,
            symbols: &symbols,
            imports: Vec::new(),
            uses_time: false,
            uses_json: false,
            body: String::new(),
            diags: &mut *diags,
            relative: &file.relative,
            known: files.iter().map(|f| f.relative.as_str()).collect(),
        };
        writer.check_imports();
        for message in &parsed.messages {
            writer.message(message, "", &parsed.package);
        }
        for e in &parsed.enums {
            writer.enumeration(e, "");
        }
        for service in &parsed.services {
            writer.service(service);
        }
        writer.descriptor(&Describer::file(&kinds, parsed, &file.relative), files, &well_known);
        let mut text = format!("// Generated by tondo from {}; it changes with the file.\n\n", file.relative);
        if !parsed.services.is_empty() {
            text.push_str("import std.grpc\n");
        }
        text.push_str("import std.protobuf\n");
        if writer.uses_json {
            text.push_str("import std.json\n");
        }
        if writer.uses_time {
            text.push_str("import std.time\n");
        }
        for (path, alias) in &writer.imports {
            let _ = writeln!(text, "import {} as {alias}", path.join("."));
        }
        text.push('\n');
        text.push_str(&writer.body);
        out.push((file.module.clone(), text));
    }
    out
}

fn collect_message(
    message: &Message,
    prefix: &str,
    outer: &str,
    module: &[String],
    symbols: &mut HashMap<String, Symbol>,
) {
    let full = format!("{prefix}{}", message.name);
    let name = type_name(format!("{outer}{}", message.name), "Message");
    symbols.insert(full.clone(), Symbol { module: module.to_vec(), name: name.clone(), kind: SymbolKind::Message });
    let inner_prefix = format!("{full}.");
    for nested in &message.messages {
        collect_message(nested, &inner_prefix, &format!("{outer}{}", message.name), module, symbols);
    }
    for field in &message.fields {
        if let Some(group) = &field.group {
            collect_message(group, &inner_prefix, &format!("{outer}{}", message.name), module, symbols);
        }
    }
    for e in &message.enums {
        collect_enum(e, &inner_prefix, &format!("{outer}{}", message.name), module, symbols);
    }
}

fn collect_enum(e: &Enum, prefix: &str, outer: &str, module: &[String], symbols: &mut HashMap<String, Symbol>) {
    let name = type_name(format!("{outer}{}", e.name), "Enum");
    let first = e.values.first().map(|(value, _, _)| variant(&e.name, value)).unwrap_or_default();
    symbols
        .insert(format!("{prefix}{}", e.name), Symbol { module: module.to_vec(), name, kind: SymbolKind::Enum(first) });
}

/// The variant of an enum's value: without the enum's name in front, in
/// Pascal case: `STATUS_ACTIVE` of `Status` is `Active`.
fn variant(enum_name: &str, value: &str) -> String {
    let prefix: String = enum_name
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            let underscore = (i > 0 && c.is_uppercase()).then_some('_');
            underscore.into_iter().chain(c.to_uppercase())
        })
        .collect();
    let stripped =
        value.strip_prefix(&format!("{prefix}_")).filter(|rest| rest.chars().next().is_some_and(|c| c.is_alphabetic()));
    let name = pascal(stripped.unwrap_or(value));
    if name == "Unrecognized" { "UnrecognizedValue".into() } else { name }
}

/// The well-known messages of `google.protobuf`, as Tondo types and the
/// encoding they need.
fn well_known(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "google.protobuf.Timestamp" => ("time.DateTime", "Default"),
        "google.protobuf.Duration" => ("time.Duration", "Default"),
        "google.protobuf.Value" => ("json.Value", "Default"),
        "google.protobuf.Struct" => ("json.Value", "Struct"),
        "google.protobuf.ListValue" => ("json.Value", "List"),
        "google.protobuf.Empty" => ("protobuf.Empty", "Default"),
        "google.protobuf.Any" => ("protobuf.Any", "Default"),
        "google.protobuf.FieldMask" => ("protobuf.FieldMask", "Default"),
        "google.protobuf.DoubleValue" => ("Float", "Wrapper"),
        "google.protobuf.FloatValue" => ("Float32", "Wrapper"),
        "google.protobuf.Int64Value" => ("Int", "Wrapper"),
        "google.protobuf.UInt64Value" => ("UInt", "Wrapper"),
        "google.protobuf.Int32Value" => ("Int32", "Wrapper"),
        "google.protobuf.UInt32Value" => ("UInt32", "Wrapper"),
        "google.protobuf.BoolValue" => ("Bool", "Wrapper"),
        "google.protobuf.StringValue" => ("String", "Wrapper"),
        "google.protobuf.BytesValue" => ("Array[Byte]", "Wrapper"),
        _ => return None,
    })
}

/// A scalar of protobuf as a Tondo type, its encoding and zero.
fn scalar(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match name {
        "int32" => ("Int32", "Default", "0"),
        "int64" => ("Int", "Default", "0"),
        "uint32" => ("UInt32", "Default", "0"),
        "uint64" => ("UInt", "Default", "0"),
        "sint32" => ("Int32", "ZigZag", "0"),
        "sint64" => ("Int", "ZigZag", "0"),
        "fixed32" => ("UInt32", "Fixed", "0"),
        "fixed64" => ("UInt", "Fixed", "0"),
        "sfixed32" => ("Int32", "Fixed", "0"),
        "sfixed64" => ("Int", "Fixed", "0"),
        "bool" => ("Bool", "Default", "false"),
        "float" => ("Float32", "Default", "0.0"),
        "double" => ("Float", "Default", "0.0"),
        "string" => ("String", "Default", "\"\""),
        "bytes" => ("Array[Byte]", "Default", "[]"),
        _ => return None,
    })
}

/// The type of a field, resolved: its Tondo type, encoding, zero and
/// whether it is a message, which has presence.
struct Resolved {
    ty: String,
    encoding: &'static str,
    zero: String,
    message: bool,
    /// A wrapper of `google.protobuf`, already optional.
    wrapper: bool,
}

struct Translator<'a> {
    file: &'a File,
    module: &'a [String],
    symbols: &'a HashMap<String, Symbol>,
    /// The modules of other files the code names, with their aliases.
    imports: Vec<(Vec<String>, String)>,
    uses_time: bool,
    uses_json: bool,
    body: String,
    diags: &'a mut Diagnostics,
    relative: &'a str,
    known: Vec<&'a str>,
}

impl Translator<'_> {
    fn check_imports(&mut self) {
        for (path, span) in &self.file.imports {
            if !path.starts_with("google/protobuf/") && !self.known.contains(&path.as_str()) {
                let message = format!(
                    "`{path}` is no `.proto` file of this package: an import names one by its path from the source root, as `{}`",
                    self.relative
                );
                self.diags.error("E1911", *span, message);
            }
        }
    }

    /// The Tondo type a name of the file refers to from a scope, as
    /// `pkg.Outer`: the innermost declaration of that name.
    fn resolve(&mut self, written: &str, scope: &str, span: Span) -> Option<Resolved> {
        if let Some((ty, encoding, zero)) = scalar(written) {
            return Some(Resolved { ty: ty.into(), encoding, zero: zero.into(), message: false, wrapper: false });
        }
        let mut candidates = Vec::new();
        match written.strip_prefix('.') {
            Some(full) => candidates.push(full.to_string()),
            None => {
                let mut scope = scope.to_string();
                loop {
                    candidates.push(if scope.is_empty() { written.to_string() } else { format!("{scope}.{written}") });
                    match scope.rfind('.') {
                        Some(at) => scope.truncate(at),
                        None if !scope.is_empty() => scope.clear(),
                        None => break,
                    }
                }
            }
        }
        for candidate in &candidates {
            if let Some((ty, encoding)) = well_known(candidate) {
                self.uses_time |= ty.starts_with("time.");
                self.uses_json |= ty.starts_with("json.");
                let wrapper = encoding == "Wrapper";
                let message = !wrapper && !ty.starts_with("json.");
                let zero = match ty {
                    "json.Value" => "json.Value.Null".to_string(),
                    _ => String::new(),
                };
                return Some(Resolved { ty: ty.into(), encoding, zero, message, wrapper });
            }
            if let Some(symbol) = self.symbols.get(candidate).cloned() {
                let ty = self.qualified(&symbol);
                return Some(match &symbol.kind {
                    SymbolKind::Message => {
                        Resolved { ty, encoding: "Default", zero: String::new(), message: true, wrapper: false }
                    }
                    SymbolKind::Enum(first) => {
                        let zero = format!("{ty}.{first}");
                        Resolved { ty, encoding: "Default", zero, message: false, wrapper: false }
                    }
                });
            }
        }
        let message = if written.contains("google.protobuf.") {
            format!(
                "`{written}` is not supported yet: the types of `google.protobuf` this has are Timestamp, Duration, Struct, Value, ListValue, Empty, Any, FieldMask and the wrappers"
            )
        } else {
            format!("`{written}` is no message or enum of this package's `.proto` files")
        };
        self.diags.error("E1911", span, message);
        None
    }

    /// A symbol as the code of this file names it: plain in its own module,
    /// or after the alias of its module.
    fn qualified(&mut self, symbol: &Symbol) -> String {
        if symbol.module == self.module {
            return symbol.name.clone();
        }
        format!("{}.{}", self.alias(&symbol.module), symbol.name)
    }

    /// The alias the code names another module by, which it imports.
    fn alias(&mut self, module: &[String]) -> String {
        match self.imports.iter().find(|(path, _)| path == module) {
            Some((_, alias)) => alias.clone(),
            None => {
                let alias = format!("__{}", module.join("_"));
                self.imports.push((module.to_vec(), alias.clone()));
                alias
            }
        }
    }

    /// `fileDescriptor()`: the file as reflection gives it to tools, with
    /// the files it imports.
    fn descriptor(&mut self, described: &Described, files: &[ProtoSource], well_known: &[(&str, Described)]) {
        let mut imports = Vec::new();
        for (path, _) in &self.file.imports {
            if let Some(other) = files.iter().find(|f| f.relative == *path) {
                if other.module.as_slice() != self.module {
                    imports.push(format!("{}.fileDescriptor()", self.alias(&other.module)));
                }
            } else if let Some((name, known)) = well_known.iter().find(|(name, _)| name == path) {
                imports.push(descriptor_literal(name, known, "[]"));
            }
        }
        let literal = descriptor_literal(self.relative, described, &format!("[{}]", imports.join(", ")));
        let _ = write!(
            self.body,
            "/// `{}` as reflection describes it: what protoc writes of it, without\n/// its options, and the files it imports.\npub fn fileDescriptor(): protobuf.FileDescriptor {{\n    {literal}\n}}\n",
            self.relative
        );
    }

    fn message(&mut self, message: &Message, outer: &str, scope: &str) {
        let full = if scope.is_empty() { message.name.clone() } else { format!("{scope}.{}", message.name) };
        let name = type_name(format!("{outer}{}", message.name), "Message");
        let mut fields = String::new();
        let mut taken: Vec<String> = Vec::new();
        for field in &message.fields {
            let Some(line) = self.field(field, &full, &name) else { continue };
            taken.push(camel(&field.name));
            fields.push_str(&line);
        }
        for oneof in &message.oneofs {
            let enum_name = type_name(format!("{name}{}", pascal(&oneof.name)), "OneOf");
            let mut variants = String::new();
            for field in &oneof.fields {
                let Some(resolved) = self.resolve(&field.ty, &full, field.type_span) else { continue };
                let encoding = annotation_encoding(resolved.encoding);
                // A wrapper in a oneof holds its value as it is.
                let ty = resolved.ty.clone();
                let _ = writeln!(
                    variants,
                    "    @protobuf.Field({}{encoding}, name: {:?}) {}({ty})",
                    field.number,
                    field.name,
                    pascal(&field.name)
                );
            }
            let _ = write!(self.body, "pub enum {enum_name} {{\n{variants}}}\n\n");
            let field_name = unique(camel(&oneof.name), &taken);
            taken.push(field_name.clone());
            let _ = writeln!(fields, "    {field_name}: {enum_name}? = none");
        }
        let unknown = unique("unknownFields".into(), &taken);
        let _ = writeln!(
            fields,
            "    @protobuf.Unknown @ignore {unknown}: protobuf.UnknownFields = protobuf.UnknownFields {{}}"
        );
        // Its scope begins with the package.
        let _ = write!(self.body, "@protobuf.Name({full:?})\npub type {name} = {{\n{fields}}}\n\n");
        let nested_outer = format!("{outer}{}", message.name);
        for nested in &message.messages {
            self.message(nested, &nested_outer, &full);
        }
        for field in &message.fields {
            if let Some(group) = &field.group {
                self.message(group, &nested_outer, &full);
            }
        }
        for e in &message.enums {
            self.enumeration(e, &nested_outer);
        }
    }

    /// A field's line of the record, or nothing when its type does not
    /// resolve.
    fn field(&mut self, field: &Field, scope: &str, message: &str) -> Option<String> {
        let syntax = self.file.syntax;
        let name = camel(&field.name);
        let renamed = match &field.json_name {
            Some(json) if *json != name => format!(" @name({json:?})"),
            _ => String::new(),
        };
        // Its name in the `.proto`, which JSON reads, when the field's is
        // another.
        let proto = if field.name == name { String::new() } else { format!(", name: {:?}", field.name) };
        if !(1..=536_870_911).contains(&field.number) {
            self.diags.error(
                "E1911",
                field.span,
                format!(
                    "the field `{}` has the number {}: a field number goes from 1 to 536870911",
                    field.name, field.number
                ),
            );
            return None;
        }
        if let Some((key, value)) = &field.map {
            let key = self.resolve(key, scope, field.span)?;
            let value = self.resolve(value, scope, field.span)?;
            let encoding = annotation_encoding(value.encoding);
            let value_ty = if value.wrapper { format!("{}?", value.ty) } else { value.ty.clone() };
            return Some(format!(
                "    @protobuf.Field({}{encoding}{proto}){renamed} {name}: Map[{}, {value_ty}] = [:]\n",
                field.number, key.ty
            ));
        }
        if field.group.is_some() {
            let ty = type_name(format!("{message}{}", field.ty), "Message");
            let shape = match field.label {
                Label::Repeated => format!("Array[{ty}] = []"),
                Label::Required => ty,
                _ => format!("{ty}? = none"),
            };
            return Some(format!(
                "    @protobuf.Field({}, encoding: protobuf.Encoding.Group{proto}){renamed} {name}: {shape}\n",
                field.number
            ));
        }
        let resolved = self.resolve(&field.ty, scope, field.type_span)?;
        let encoding = annotation_encoding(resolved.encoding);
        let numeric = !resolved.message
            && !resolved.wrapper
            && !matches!(resolved.ty.as_str(), "String" | "Array[Byte]" | "json.Value");
        let packed = match (field.label, field.packed, syntax) {
            (Label::Repeated, Some(false), _) | (Label::Repeated, None, Syntax::Proto2) if numeric => ", packed: false",
            _ => "",
        };
        let ty = &resolved.ty;
        let shape = match field.label {
            Label::Repeated => format!("Array[{ty}] = []"),
            _ if resolved.wrapper => format!("{ty}? = none"),
            Label::Required => ty.clone(),
            _ if resolved.message => format!("{ty}? = none"),
            Label::Optional if syntax == Syntax::Proto3 => format!("{ty}? = none"),
            // proto2's `[default = v]` is the field's default, without presence.
            Label::Optional | Label::None if syntax == Syntax::Proto2 => match &field.default {
                Some(default) => format!("{ty} = {}", self.default_value(default, &resolved, field)),
                None => format!("{ty}? = none"),
            },
            _ => format!("{ty} = {}", resolved.zero),
        };
        Some(format!("    @protobuf.Field({}{encoding}{packed}{proto}){renamed} {name}: {shape}\n", field.number))
    }

    /// The Tondo value of a proto2 default.
    fn default_value(&mut self, default: &Token, resolved: &Resolved, field: &Field) -> String {
        match (default, resolved.ty.as_str()) {
            (Token::Text(text), "String") => format!("{text:?}").replace('{', "{{").replace('}', "}}"),
            (Token::Text(text), "Array[Byte]") => {
                format!("[{}]", text.chars().map(|c| (c as u32 & 0xff).to_string()).collect::<Vec<_>>().join(", "))
            }
            (Token::Int(value), "Float" | "Float32") => format!("{value}.0"),
            (Token::Int(value), _) => value.to_string(),
            (Token::Float(text), _) => text.clone(),
            // `inf`, `-inf` and `nan` are names to the tokens.
            (Token::Name(name), ty @ ("Float" | "Float32")) => match name.as_str() {
                "inf" => format!("{ty}.infinity"),
                "-inf" => format!("-{ty}.infinity"),
                _ => format!("{ty}.nan"),
            },
            (Token::Name(name), "Bool") => name.clone(),
            (Token::Name(name), ty) => {
                let enum_name = ty.rsplit('.').next().unwrap_or(ty);
                let original = self
                    .symbols
                    .iter()
                    .find(|(_, s)| s.name == enum_name)
                    .map(|(full, _)| full.rsplit('.').next().unwrap_or("").to_string())
                    .unwrap_or_default();
                format!("{ty}.{}", variant(&original, name))
            }
            _ => {
                self.diags.error(
                    "E1911",
                    field.span,
                    format!("the default of `{}` is not a value of its type", field.name),
                );
                resolved.zero.clone()
            }
        }
    }

    fn enumeration(&mut self, e: &Enum, outer: &str) {
        let name = type_name(format!("{outer}{}", e.name), "Enum");
        if e.values.is_empty() {
            self.diags.error("E1911", e.span, format!("the enum `{}` has no value", e.name));
            return;
        }
        let mut text = format!("pub enum {name} {{\n");
        let mut shown = format!(
            "/// The name of the value in the `.proto` file.\nimpl Display for {name} {{\n    fn display(self): String {{\n        match self {{\n"
        );
        let mut seen: Vec<String> = Vec::new();
        for (value, number, _) in &e.values {
            let variant_name = unique(variant(&e.name, value), &seen);
            seen.push(variant_name.clone());
            let _ = writeln!(text, "    @protobuf.Value({number}) @name({value:?}) {variant_name}");
            let _ = writeln!(shown, "            {name}.{variant_name} => {value:?}");
        }
        // proto3's enums are open: they keep a number they do not know.
        if self.file.syntax == Syntax::Proto3 {
            text.push_str("    Unrecognized(Int32)\n");
            let _ = writeln!(shown, "            {name}.Unrecognized(number) => \"{{number}}\"");
        }
        text.push_str("}\n\n");
        shown.push_str("        }\n    }\n}\n\n");
        self.body.push_str(&text);
        self.body.push_str(&shown);
    }
}

impl Translator<'_> {
    /// A service: the trait of its calls, the function that makes a server's
    /// `grpc.Service` of a value that implements it, and its client.
    fn service(&mut self, service: &Service) {
        let package = self.file.package.clone();
        let full = if package.is_empty() { service.name.clone() } else { format!("{package}.{}", service.name) };
        let name = type_name(service.name.clone(), "Service");
        let client = format!("{name}Client");
        let mut calls = String::new();
        let mut methods = Vec::new();
        let mut client_methods = String::new();
        for method in &service.methods {
            let (Some(input), Some(output)) = (
                self.resolve(&method.input, &package, method.span),
                self.resolve(&method.output, &package, method.span),
            ) else {
                continue;
            };
            if !input.message || !output.message {
                self.diags.error("E1911", method.span, format!("the call `{}` takes and gives messages", method.name));
                continue;
            }
            let (input, output) = (input.ty, output.ty);
            let call = camel(&method.name);
            let path = format!("/{full}/{}", method.name);
            let (signature, handler, client_signature, client_body) = match (
                method.client_streams,
                method.server_streams,
            ) {
                (false, false) => (
                    format!("fn {call}(self, request: {input}): {output} ! grpc.Status"),
                    format!("grpc.unary[{input}, {output}](call, (request) {{ implementation.{call}(request)? }})?"),
                    format!(
                        "{call}(self, request: {input}, metadata: Map[String, String] = [:]): {output} ! grpc.Status"
                    ),
                    format!("self.connection.unary[{input}, {output}]({path:?}, request, metadata)"),
                ),
                (false, true) => (
                    format!("fn {call}(self, request: {input}, out: Sender[{output}]): !grpc.Status"),
                    format!(
                        "grpc.serverStream[{input}, {output}](call, (request, out) {{ implementation.{call}(request, out)? }})?"
                    ),
                    format!(
                        "{call}(self, request: {input}, metadata: Map[String, String] = [:]): grpc.Stream[{output}]"
                    ),
                    format!("self.connection.serverStream[{input}, {output}]({path:?}, request, metadata)"),
                ),
                (true, false) => (
                    format!("fn {call}(self, input: grpc.Stream[{input}]): {output} ! grpc.Status"),
                    format!("grpc.clientStream[{input}, {output}](call, (input) {{ implementation.{call}(input)? }})?"),
                    format!("{call}(self, metadata: Map[String, String] = [:]): grpc.Upload[{input}, {output}]"),
                    format!("self.connection.clientStream[{input}, {output}]({path:?}, metadata)"),
                ),
                (true, true) => (
                    format!("fn {call}(self, input: grpc.Stream[{input}], out: Sender[{output}]): !grpc.Status"),
                    format!(
                        "grpc.bidiStream[{input}, {output}](call, (input, out) {{ implementation.{call}(input, out)? }})?"
                    ),
                    format!("{call}(self, metadata: Map[String, String] = [:]): grpc.Duplex[{input}, {output}]"),
                    format!("self.connection.bidiStream[{input}, {output}]({path:?}, metadata)"),
                ),
            };
            let _ = writeln!(calls, "    {signature}");
            methods.push(format!(
                "            grpc.Method {{ name: {:?}, handler: (call) {{ {handler} }} }},",
                method.name
            ));
            let _ = write!(
                client_methods,
                "/// The call `{path}`.\npub fn {client}.{client_signature} {{\n    {client_body}\n}}\n\n"
            );
        }
        let function = camel(&name);
        let _ = write!(
            self.body,
            "/// The calls of the service `{full}`, which a server implements.\npub trait {name} {{\n{calls}}}\n\n/// The service `{full}` of `implementation`, which `app.grpc` serves.\npub fn {function}[T: {name} + Send + Share](implementation: T): grpc.Service {{\n    grpc.Service {{\n        name: {full:?}\n        methods: [\n{}\n        ]\n        files: [fileDescriptor()]\n    }}\n}}\n\n/// A client of the service `{full}`.\npub type {client} = {{\n    priv connection: grpc.Channel\n}}\n\n/// A client of the service at `target`, `http://host:port`, or\n/// `https://host:port` for TLS.\npub fn {client}.connect(target: String, trust: String = \"\"): {client} ! grpc.Status {{\n    {client} {{ connection: grpc.connect(target, trust: trust)? }}\n}}\n\n/// A client on a channel that other clients share.\npub fn {client}.on(shared: grpc.Channel): {client} {{\n    {client} {{ connection: shared }}\n}}\n\n{client_methods}",
            methods.join("\n")
        );
    }
}

fn annotation_encoding(encoding: &str) -> String {
    match encoding {
        "Default" => String::new(),
        other => format!(", encoding: protobuf.Encoding.{other}"),
    }
}

/// A name that none of `taken` is, with `_` after it if it must.
fn unique(name: String, taken: &[String]) -> String {
    let mut name = name;
    while taken.contains(&name) {
        name.push('_');
    }
    name
}

// ---------------------------------------------------------------------------
// Descriptors, which reflection gives to tools
// ---------------------------------------------------------------------------

/// The files of `google.protobuf` a file may import, as their messages:
/// what reflection gives of them, without their options.
const WELL_KNOWN_FILES: [(&str, &str); 7] = [
    ("google/protobuf/any.proto", "message Any { string type_url = 1; bytes value = 2; }"),
    ("google/protobuf/duration.proto", "message Duration { int64 seconds = 1; int32 nanos = 2; }"),
    ("google/protobuf/empty.proto", "message Empty {}"),
    ("google/protobuf/field_mask.proto", "message FieldMask { repeated string paths = 1; }"),
    (
        "google/protobuf/struct.proto",
        "message Struct { map<string, Value> fields = 1; } message Value { oneof kind { NullValue null_value = 1; \
         double number_value = 2; string string_value = 3; bool bool_value = 4; Struct struct_value = 5; \
         ListValue list_value = 6; } } enum NullValue { NULL_VALUE = 0; } message ListValue { repeated Value values = 1; }",
    ),
    ("google/protobuf/timestamp.proto", "message Timestamp { int64 seconds = 1; int32 nanos = 2; }"),
    (
        "google/protobuf/wrappers.proto",
        "message DoubleValue { double value = 1; } message FloatValue { float value = 1; } message Int64Value { \
         int64 value = 1; } message UInt64Value { uint64 value = 1; } message Int32Value { int32 value = 1; } \
         message UInt32Value { uint32 value = 1; } message BoolValue { bool value = 1; } message StringValue { \
         string value = 1; } message BytesValue { bytes value = 1; }",
    ),
];

/// A file as reflection gives it: its `FileDescriptorProto`, as protoc
/// writes it without the options, and the full names it declares.
struct Described {
    bytes: Vec<u8>,
    symbols: Vec<String>,
}

/// Protobuf's wire format, enough to write descriptors.
#[derive(Default)]
struct Wire(Vec<u8>);

impl Wire {
    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    fn int(&mut self, number: u32, value: i64) {
        self.varint(u64::from(number << 3));
        self.varint(value as u64);
    }

    fn bytes(&mut self, number: u32, value: &[u8]) {
        self.varint(u64::from(number << 3 | 2));
        self.varint(value.len() as u64);
        self.0.extend_from_slice(value);
    }

    fn text(&mut self, number: u32, value: &str) {
        self.bytes(number, value.as_bytes());
    }
}

/// Writes descriptors; `kinds` says of each full name whether it is an
/// enum.
struct Describer<'a> {
    kinds: &'a HashMap<String, bool>,
    syntax: Syntax,
    symbols: Vec<String>,
}

impl Describer<'_> {
    fn file(kinds: &HashMap<String, bool>, file: &File, name: &str) -> Described {
        let mut describer = Describer { kinds, syntax: file.syntax, symbols: Vec::new() };
        let package = file.package.as_str();
        let mut out = Wire::default();
        out.text(1, name);
        if !package.is_empty() {
            out.text(2, package);
        }
        for (import, _) in &file.imports {
            out.text(3, import);
        }
        for message in &file.messages {
            out.bytes(4, &describer.message(message, package));
        }
        for e in &file.enums {
            out.bytes(5, &describer.enumeration(e, package));
        }
        for service in &file.services {
            out.bytes(6, &describer.service(service, package));
        }
        for (extendee, field) in &file.extensions {
            out.bytes(7, &describer.extension(extendee, field, package));
        }
        describer.options(&mut out, 8, &file.options, FILE_OPTIONS);
        for &index in &file.public {
            out.int(10, index as i64);
        }
        for &index in &file.weak {
            out.int(11, index as i64);
        }
        if file.syntax == Syntax::Proto3 {
            out.text(12, "proto3");
        }
        Described { bytes: out.0, symbols: describer.symbols }
    }

    fn message(&mut self, message: &Message, scope: &str) -> Vec<u8> {
        let full = qualified(scope, &message.name);
        self.symbols.push(full.clone());
        let mut out = Wire::default();
        out.text(1, &message.name);
        // The fields in the order they are written, a oneof's among them.
        let mut fields: Vec<(&Field, Option<usize>)> = message.fields.iter().map(|f| (f, None)).collect();
        for (index, oneof) in message.oneofs.iter().enumerate() {
            fields.extend(oneof.fields.iter().map(|f| (f, Some(index))));
        }
        fields.sort_by_key(|(field, _)| field.span.start);
        // A proto3 `optional` is in a oneof of its own, after the others.
        let mut synthetic = Vec::new();
        for (field, oneof) in &fields {
            let optional = self.syntax == Syntax::Proto3 && field.label == Label::Optional;
            let index = match oneof {
                Some(index) => Some(*index),
                None if optional => {
                    synthetic.push(format!("_{}", field.name));
                    Some(message.oneofs.len() + synthetic.len() - 1)
                }
                None => None,
            };
            out.bytes(2, &self.field(field, &full, index, optional, None));
        }
        // The types inside, in the order they are written: messages, and
        // those of its maps and groups.
        let mut nested: Vec<(u32, Vec<u8>)> = Vec::new();
        for inner in &message.messages {
            nested.push((inner.span.start, self.message(inner, &full)));
        }
        for field in &message.fields {
            if let Some(group) = &field.group {
                nested.push((field.span.start, self.message(group, &full)));
            }
            if let Some((key, value)) = &field.map {
                nested.push((field.span.start, self.entry(field, key, value, &full)));
            }
        }
        nested.sort_by_key(|(at, _)| *at);
        for (_, bytes) in nested {
            out.bytes(3, &bytes);
        }
        for e in &message.enums {
            out.bytes(4, &self.enumeration(e, &full));
        }
        // Extension ranges end after their last number, as reserved ones do.
        for &(start, end) in &message.extension_ranges {
            out.bytes(5, &range(start, end + 1));
        }
        for (extendee, field) in &message.extensions {
            out.bytes(6, &self.extension(extendee, field, &full));
        }
        self.options(&mut out, 7, &message.options, MESSAGE_OPTIONS);
        for oneof in &message.oneofs {
            let mut decl = Wire::default();
            decl.text(1, &oneof.name);
            out.bytes(8, &decl.0);
        }
        for name in synthetic {
            let mut decl = Wire::default();
            decl.text(1, &name);
            out.bytes(8, &decl.0);
        }
        for &(start, end) in &message.reserved.ranges {
            out.bytes(9, &range(start, end + 1));
        }
        for name in &message.reserved.names {
            out.text(10, name);
        }
        out.0
    }

    /// The message protoc makes of a map field: `TagsEntry`, its key and
    /// value.
    fn entry(&mut self, field: &Field, key: &str, value: &str, scope: &str) -> Vec<u8> {
        let name = entry_name(&field.name);
        let full = qualified(scope, &name);
        let mut out = Wire::default();
        out.text(1, &name);
        for (number, part, ty) in [(1, "key", key), (2, "value", value)] {
            let part = Field {
                label: Label::None,
                ty: ty.into(),
                map: None,
                name: part.into(),
                number,
                span: field.span,
                type_span: field.type_span,
                default: None,
                packed: None,
                json_name: None,
                group: None,
                options: Vec::new(),
            };
            out.bytes(2, &self.field(&part, &full, None, false, None));
        }
        let mut options = Wire::default();
        options.int(7, 1);
        out.bytes(7, &options.0);
        out.0
    }

    /// A field of an `extend` block, named in its scope.
    fn extension(&mut self, extendee: &str, field: &Field, scope: &str) -> Vec<u8> {
        self.symbols.push(qualified(scope, &field.name));
        let extended = self.full_name(extendee, scope).map_or_else(|| extendee.to_string(), |(name, _)| name);
        self.field(field, scope, None, false, Some(&extended))
    }

    fn field(
        &self,
        field: &Field,
        scope: &str,
        oneof: Option<usize>,
        optional: bool,
        extendee: Option<&str>,
    ) -> Vec<u8> {
        let mut out = Wire::default();
        out.text(1, &field.name);
        if let Some(extendee) = extendee {
            out.text(2, &format!(".{extendee}"));
        }
        out.int(3, field.number as i64);
        let label = match (field.label, &field.map) {
            (_, Some(_)) | (Label::Repeated, _) => 3,
            (Label::Required, _) => 2,
            _ => 1,
        };
        out.int(4, label);
        let (kind, type_name) = if field.map.is_some() {
            (11, Some(qualified(scope, &entry_name(&field.name))))
        } else if field.group.is_some() {
            (10, Some(qualified(scope, &field.ty)))
        } else if let Some(kind) = scalar_kind(&field.ty) {
            (kind, None)
        } else {
            match self.full_name(&field.ty, scope) {
                Some((full, true)) => (14, Some(full)),
                Some((full, false)) => (11, Some(full)),
                None => (11, Some(field.ty.clone())),
            }
        };
        out.int(5, kind);
        if let Some(name) = &type_name {
            out.text(6, &format!(".{name}"));
        }
        if let Some(default) = &field.default {
            out.text(7, &default_text(default, kind));
        }
        self.options(&mut out, 8, &field.options, FIELD_OPTIONS);
        if let Some(index) = oneof {
            out.int(9, index as i64);
        }
        out.text(10, field.json_name.as_deref().unwrap_or(&json_name(&field.name)));
        if optional {
            out.int(17, 1);
        }
        out.0
    }

    fn enumeration(&mut self, e: &Enum, scope: &str) -> Vec<u8> {
        self.symbols.push(qualified(scope, &e.name));
        let mut out = Wire::default();
        out.text(1, &e.name);
        for (index, (name, number, _)) in e.values.iter().enumerate() {
            let mut value = Wire::default();
            value.text(1, name);
            value.int(2, *number as i64);
            if let Some(options) = e.value_options.get(index) {
                self.options(&mut value, 3, options, ENUM_VALUE_OPTIONS);
            }
            out.bytes(2, &value.0);
        }
        self.options(&mut out, 3, &e.options, ENUM_OPTIONS);
        // An enum's reserved ranges include their end.
        for &(start, end) in &e.reserved.ranges {
            out.bytes(4, &range(start, end));
        }
        for name in &e.reserved.names {
            out.text(5, name);
        }
        out.0
    }

    fn service(&mut self, service: &Service, package: &str) -> Vec<u8> {
        let full = qualified(package, &service.name);
        self.symbols.push(full.clone());
        let mut out = Wire::default();
        out.text(1, &service.name);
        for method in &service.methods {
            self.symbols.push(format!("{full}.{}", method.name));
            let mut described = Wire::default();
            described.text(1, &method.name);
            for (number, written) in [(2, &method.input), (3, &method.output)] {
                let name = self.full_name(written, package).map_or_else(|| written.clone(), |(name, _)| name);
                described.text(number, &format!(".{name}"));
            }
            self.options(&mut described, 4, &method.options, METHOD_OPTIONS);
            if method.client_streams {
                described.int(5, 1);
            }
            if method.server_streams {
                described.int(6, 1);
            }
            out.bytes(2, &described.0);
        }
        self.options(&mut out, 3, &service.options, SERVICE_OPTIONS);
        out.0
    }

    /// The options of `descriptor.proto` among some, as the message of
    /// field `number`, in the order of their numbers; nothing when there
    /// are none. Options of their own, `(name)`, are left out.
    fn options(&self, out: &mut Wire, number: u32, written: &Options, known: &[(&str, u32, Value)]) {
        let mut options: Vec<(u32, Vec<u8>)> = Vec::new();
        for (name, token) in written {
            let Some(&(_, field, value)) = known.iter().find(|(known, _, _)| known == name) else { continue };
            let mut one = Wire::default();
            match (value, token) {
                (Value::Text, Token::Text(text)) => one.text(field, text),
                (Value::Bool, Token::Name(word)) => one.int(field, (word == "true") as i64),
                (Value::Enum(values), Token::Name(word)) => {
                    let Some(&(_, at)) = values.iter().find(|(name, _)| name == word) else { continue };
                    one.int(field, at);
                }
                (Value::Enum(_), Token::Int(at)) => one.int(field, *at as i64),
                _ => continue,
            }
            options.push((field, one.0));
        }
        if options.is_empty() {
            return;
        }
        options.sort_by_key(|(field, _)| *field);
        let bytes: Vec<u8> = options.into_iter().flat_map(|(_, bytes)| bytes).collect();
        out.bytes(number, &bytes);
    }

    /// The full name a type written in a scope names, and whether it is an
    /// enum, as protoc finds it: from the innermost scope out.
    fn full_name(&self, written: &str, scope: &str) -> Option<(String, bool)> {
        if let Some(full) = written.strip_prefix('.') {
            return self.kinds.get(full).map(|&e| (full.to_string(), e));
        }
        let mut scope = scope.to_string();
        loop {
            let candidate = qualified(&scope, written);
            if let Some(&e) = self.kinds.get(&candidate) {
                return Some((candidate, e));
            }
            if scope.is_empty() {
                return None;
            }
            match scope.rfind('.') {
                Some(at) => scope.truncate(at),
                None => scope.clear(),
            }
        }
    }
}

/// `DescriptorProto.ReservedRange` and its kind: a start and an end.
fn range(start: i128, end: i128) -> Vec<u8> {
    let mut out = Wire::default();
    out.int(1, start as i64);
    out.int(2, end as i64);
    out.0
}

/// What an option of `descriptor.proto` holds.
#[derive(Clone, Copy)]
enum Value {
    Text,
    Bool,
    Enum(&'static [(&'static str, i64)]),
}

const FILE_OPTIONS: &[(&str, u32, Value)] = &[
    ("java_package", 1, Value::Text),
    ("java_outer_classname", 8, Value::Text),
    ("optimize_for", 9, Value::Enum(&[("SPEED", 1), ("CODE_SIZE", 2), ("LITE_RUNTIME", 3)])),
    ("java_multiple_files", 10, Value::Bool),
    ("go_package", 11, Value::Text),
    ("cc_generic_services", 16, Value::Bool),
    ("java_generic_services", 17, Value::Bool),
    ("py_generic_services", 18, Value::Bool),
    ("java_generate_equals_and_hash", 20, Value::Bool),
    ("deprecated", 23, Value::Bool),
    ("java_string_check_utf8", 27, Value::Bool),
    ("cc_enable_arenas", 31, Value::Bool),
    ("objc_class_prefix", 36, Value::Text),
    ("csharp_namespace", 37, Value::Text),
    ("swift_prefix", 39, Value::Text),
    ("php_class_prefix", 40, Value::Text),
    ("php_namespace", 41, Value::Text),
    ("php_metadata_namespace", 44, Value::Text),
    ("ruby_package", 45, Value::Text),
];

const MESSAGE_OPTIONS: &[(&str, u32, Value)] = &[
    ("message_set_wire_format", 1, Value::Bool),
    ("no_standard_descriptor_accessor", 2, Value::Bool),
    ("deprecated", 3, Value::Bool),
    ("map_entry", 7, Value::Bool),
    ("deprecated_legacy_json_field_conflicts", 11, Value::Bool),
];

const FIELD_OPTIONS: &[(&str, u32, Value)] = &[
    ("ctype", 1, Value::Enum(&[("STRING", 0), ("CORD", 1), ("STRING_PIECE", 2)])),
    ("packed", 2, Value::Bool),
    ("deprecated", 3, Value::Bool),
    ("lazy", 5, Value::Bool),
    ("jstype", 6, Value::Enum(&[("JS_NORMAL", 0), ("JS_STRING", 1), ("JS_NUMBER", 2)])),
    ("weak", 10, Value::Bool),
    ("unverified_lazy", 15, Value::Bool),
    ("debug_redact", 16, Value::Bool),
    ("retention", 17, Value::Enum(&[("RETENTION_UNKNOWN", 0), ("RETENTION_RUNTIME", 1), ("RETENTION_SOURCE", 2)])),
];

const ENUM_OPTIONS: &[(&str, u32, Value)] = &[("allow_alias", 2, Value::Bool), ("deprecated", 3, Value::Bool)];

const ENUM_VALUE_OPTIONS: &[(&str, u32, Value)] = &[("deprecated", 1, Value::Bool), ("debug_redact", 3, Value::Bool)];

const SERVICE_OPTIONS: &[(&str, u32, Value)] = &[("deprecated", 33, Value::Bool)];

const METHOD_OPTIONS: &[(&str, u32, Value)] = &[
    ("deprecated", 33, Value::Bool),
    ("idempotency_level", 34, Value::Enum(&[("IDEMPOTENCY_UNKNOWN", 0), ("NO_SIDE_EFFECTS", 1), ("IDEMPOTENT", 2)])),
];

/// A file's `protobuf.FileDescriptor` as Tondo writes it.
fn descriptor_literal(name: &str, described: &Described, imports: &str) -> String {
    let symbols: Vec<String> = described.symbols.iter().map(|s| format!("{s:?}")).collect();
    let bytes: Vec<String> = described.bytes.iter().map(u8::to_string).collect();
    format!(
        "protobuf.FileDescriptor {{ name: {name:?}, symbols: [{}], bytes: [{}], imports: {imports} }}",
        symbols.join(", "),
        bytes.join(", ")
    )
}

fn qualified(scope: &str, name: &str) -> String {
    if scope.is_empty() { name.to_string() } else { format!("{scope}.{name}") }
}

/// A field's name in JSON, as protoc gives it: `user_id` is `userId`.
fn json_name(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The message protoc makes of a map field: `tag_names` is
/// `TagNamesEntry`.
fn entry_name(field: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in field.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out + "Entry"
}

/// A scalar's `FieldDescriptorProto.Type`.
fn scalar_kind(name: &str) -> Option<i64> {
    Some(match name {
        "double" => 1,
        "float" => 2,
        "int64" => 3,
        "uint64" => 4,
        "int32" => 5,
        "fixed64" => 6,
        "fixed32" => 7,
        "bool" => 8,
        "string" => 9,
        "bytes" => 12,
        "uint32" => 13,
        "sfixed32" => 15,
        "sfixed64" => 16,
        "sint32" => 17,
        "sint64" => 18,
        _ => return None,
    })
}

/// A default as protoc writes it in a descriptor: a number in decimal, a
/// float as C's `%g` gives it, a string as it is and bytes escaped as C
/// escapes them.
fn default_text(value: &Token, kind: i64) -> String {
    match (value, kind) {
        // The tokens keep each byte of a `bytes` constant as a character.
        (Token::Text(text), 12) => text
            .chars()
            .map(|c| c as u32 as u8)
            .map(|b| match b {
                b'\n' => "\\n".to_string(),
                b'\r' => "\\r".to_string(),
                b'\t' => "\\t".to_string(),
                b'"' => "\\\"".to_string(),
                b'\'' => "\\'".to_string(),
                b'\\' => "\\\\".to_string(),
                0x20..=0x7e => (b as char).to_string(),
                _ => format!("\\{b:03o}"),
            })
            .collect(),
        (Token::Text(text), _) => text.clone(),
        (Token::Int(number), 1 | 2) => shortest(*number as f64, kind == 2),
        (Token::Float(text), _) => match text.parse::<f64>() {
            Ok(number) => shortest(number, kind == 2),
            Err(_) => text.clone(),
        },
        (Token::Int(number), _) => number.to_string(),
        (Token::Name(name), _) => name.clone(),
        (Token::Symbol(c), _) => c.to_string(),
    }
}

/// A float as protoc's `SimpleDtoa` (or `SimpleFtoa`, for a `float`)
/// writes it: `%g` with the fewest digits of 15 and 17 (6 and 9) that
/// read back the same.
fn shortest(value: f64, single: bool) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let value = if single { value as f32 as f64 } else { value };
    let (first, last) = if single { (6, 9) } else { (15, 17) };
    for precision in [first, last] {
        let text = c_g(value, precision);
        let back = text.parse::<f64>().unwrap_or(f64::NAN);
        let same = if single { back as f32 == value as f32 } else { back == value };
        if same || precision == last {
            return text;
        }
    }
    unreachable!()
}

/// C's `%.<precision>g`.
fn c_g(value: f64, precision: usize) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let scientific = format!("{:.*e}", precision - 1, value);
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let trim = |text: String| -> String {
        if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
    };
    if exponent < -4 || exponent >= precision as i32 {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa.to_string()), exponent.abs())
    } else {
        let decimals = (precision as i32 - 1 - exponent).max(0) as usize;
        trim(format!("{value:.decimals$}"))
    }
}
