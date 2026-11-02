//! The expressions the debugger evaluates (toolchain 7.6), in `print`, a
//! breakpoint's condition, a logpoint's message and a watch: bindings and
//! their parts, literals, arithmetic, comparisons, `and`, `or`, `not`, and
//! methods that read, as `length()`. They read values as the debugger shows
//! them, and never run the program's code, so they change nothing.

use std::collections::HashMap;

use super::super::shown;

/// An expression, as `parse` reads it.
#[derive(Debug)]
pub(super) enum Expr {
    Literal(Value),
    /// A binding, or a name that is not one, as the type of `Role.Admin`.
    Name(String),
    /// `.field`, or `.0` of a tuple or a variant.
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Method(Box<Expr>, String, Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
}

/// A value: a number, a truth, a text or a character, or anything else as
/// the debugger shows it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    Int(i128),
    Float(f64),
    Bool(bool),
    Text(String),
    Char(char),
    Shown(String),
}

impl Value {
    /// The value as the debugger shows it.
    pub(super) fn shown(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Float(x) => format!("{x:?}"),
            Value::Bool(b) => b.to_string(),
            Value::Text(text) => format!("{text:?}"),
            Value::Char(c) => format!("'{c}'"),
            Value::Shown(text) => text.clone(),
        }
    }

    /// The type a computed value has, when it is one of those it computes.
    pub(super) fn ty(&self) -> Option<&'static str> {
        Some(match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Bool(_) => "Bool",
            Value::Text(_) => "String",
            Value::Char(_) => "Char",
            Value::Shown(_) => return None,
        })
    }

    /// What a value's text, as the debugger shows it, holds.
    pub(super) fn from_shown(text: &str) -> Value {
        if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'))
            && let Some(text) = unescaped(inner, '"')
        {
            return Value::Text(text);
        }
        if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
            let mut chars = inner.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                return Value::Char(c);
            }
        }
        match text {
            "true" => return Value::Bool(true),
            "false" => return Value::Bool(false),
            _ => {}
        }
        if let Ok(n) = text.parse::<i128>() {
            return Value::Int(n);
        }
        let numeric = text.starts_with(|c: char| c.is_ascii_digit() || c == '-') || text == "NaN";
        match text.parse::<f64>() {
            Ok(x) if numeric => Value::Float(x),
            _ => Value::Shown(text.to_string()),
        }
    }

    /// A text without its quotes, as a logpoint writes it.
    pub(super) fn written(&self) -> String {
        match self {
            Value::Text(text) => text.clone(),
            Value::Char(c) => c.to_string(),
            other => other.shown(),
        }
    }

    /// The name of the kind, for messages.
    fn kind(&self) -> &'static str {
        match self {
            Value::Shown(text) if collection(text) => "a collection",
            Value::Shown(_) => "this value",
            value => match value.ty() {
                Some("Int") => "an Int",
                Some("Float") => "a Float",
                Some("Bool") => "a Bool",
                Some("String") => "a String",
                _ => "a Char",
            },
        }
    }
}

/// What is between quotes, without its escapes; `None` when a quote
/// ends it before.
fn unescaped(inner: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == quote {
            return None;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            'u' => {
                let rest = chars.as_str().strip_prefix('{')?;
                let close = rest.find('}')?;
                out.push(char::from_u32(u32::from_str_radix(&rest[..close], 16).ok()?)?);
                chars = rest[close + 1..].chars();
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// Whether a value's text is an array's, a map's or a set's.
fn collection(text: &str) -> bool {
    text.starts_with('[') || text.starts_with("Set[")
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Token {
    Name(String),
    Int(i128),
    Float(f64),
    Text(String),
    Char(char),
    Op(&'static str),
}

const OPERATORS: [&str; 17] = ["==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "(", ")", "[", "]", ".", ","];

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while let Some(c) = rest.chars().next() {
        let after_dot = out.last() == Some(&Token::Op("."));
        let end = if c.is_alphabetic() || c == '_' {
            let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
            out.push(Token::Name(rest[..end].to_string()));
            end
        } else if c.is_ascii_digit() && after_dot {
            // A tuple's or a variant's part, `.0`.
            let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            out.push(Token::Name(rest[..end].to_string()));
            end
        } else if c.is_ascii_digit() {
            let (token, end) = number(rest)?;
            out.push(token);
            end
        } else if c == '"' || c == '\'' {
            let end = quoted_end(rest).ok_or_else(|| format!("{c} without its closing {c}: `{rest}`"))?;
            let inner = unescaped(&rest[1..end - 1], c).ok_or_else(|| format!("`{}` is not a text", &rest[..end]))?;
            match c {
                '"' => out.push(Token::Text(inner)),
                _ => {
                    let mut chars = inner.chars();
                    let (Some(c), None) = (chars.next(), chars.next()) else {
                        return Err(format!("`{}` is not one character", &rest[..end]));
                    };
                    out.push(Token::Char(c));
                }
            }
            end
        } else {
            let Some(op) = OPERATORS.iter().find(|op| rest.starts_with(**op)) else {
                return Err(match c {
                    '&' | '|' if rest[1..].starts_with(c) => {
                        format!("Tondo writes `{}`", if c == '&' { "and" } else { "or" })
                    }
                    '!' => "Tondo writes `not`".to_string(),
                    _ => format!("`{c}` does not start anything an expression has"),
                });
            };
            out.push(Token::Op(op));
            op.len()
        };
        rest = rest[end..].trim_start();
    }
    Ok(out)
}

/// A number from the start of `text`, and where it ends: an integer, in
/// decimal or with `0x`, `0o` or `0b`, or a float.
fn number(text: &str) -> Result<(Token, usize), String> {
    let bytes = text.as_bytes();
    let word = |from: usize| {
        let mut end = from;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric()
                || bytes[end] == b'_'
                || (matches!(bytes[end], b'+' | b'-')
                    && matches!(bytes[end - 1], b'e' | b'E')
                    && !text.starts_with("0x")))
        {
            end += 1;
        }
        end
    };
    let mut end = word(0);
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end = word(end + 1);
    }
    let digits = text[..end].replace('_', "");
    let radix = [("0x", 16), ("0o", 8), ("0b", 2)].into_iter().find(|(prefix, _)| digits.starts_with(prefix));
    let token = match radix {
        Some((prefix, radix)) => i128::from_str_radix(&digits[prefix.len()..], radix).ok().map(Token::Int),
        None => match digits.parse::<i128>() {
            Ok(n) => Some(Token::Int(n)),
            Err(_) => digits.parse::<f64>().ok().map(Token::Float),
        },
    };
    token.map(|token| (token, end)).ok_or_else(|| format!("`{}` is not a number", &text[..end]))
}

/// Where a text or a character in quotes ends, past its escapes.
fn quoted_end(text: &str) -> Option<usize> {
    let quote = text.as_bytes()[0];
    let mut escaped = false;
    for (index, &byte) in text.as_bytes().iter().enumerate().skip(1) {
        match byte {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            _ if byte == quote => return Some(index + 1),
            _ => {}
        }
    }
    None
}

/// Reads an expression.
pub(super) fn parse(text: &str) -> Result<Expr, String> {
    let tokens = tokens(text)?;
    if tokens.is_empty() {
        return Err("an expression is missing".into());
    }
    let mut parser = Parser { tokens, at: 0 };
    let expr = parser.or()?;
    match parser.tokens.get(parser.at) {
        None => Ok(expr),
        Some(token) => Err(format!("`{}` does not follow there in `{text}`", written(token))),
    }
}

fn written(token: &Token) -> String {
    match token {
        Token::Name(name) => name.clone(),
        Token::Int(n) => n.to_string(),
        Token::Float(x) => x.to_string(),
        Token::Text(text) => format!("{text:?}"),
        Token::Char(c) => format!("'{c}'"),
        Token::Op(op) => op.to_string(),
    }
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek_op(&self, ops: &[&'static str]) -> Option<&'static str> {
        match self.tokens.get(self.at) {
            Some(Token::Op(op)) => ops.iter().find(|o| *o == op).copied(),
            Some(Token::Name(name)) => ops.iter().find(|o| *o == name).copied(),
            _ => None,
        }
    }

    fn expect(&mut self, op: &'static str) -> Result<(), String> {
        match self.peek_op(&[op]) {
            Some(_) => {
                self.at += 1;
                Ok(())
            }
            None => Err(match self.tokens.get(self.at) {
                Some(token) => format!("`{op}` is missing before `{}`", written(token)),
                None => format!("`{op}` is missing at the end"),
            }),
        }
    }

    /// Operators of one level, from the left; a comparison takes one.
    fn level(
        &mut self,
        ops: &[&'static str],
        once: bool,
        next: fn(&mut Parser) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let mut left = next(self)?;
        while let Some(op) = self.peek_op(ops) {
            self.at += 1;
            left = Expr::Binary(op, Box::new(left), Box::new(next(self)?));
            if once {
                break;
            }
        }
        Ok(left)
    }

    fn or(&mut self) -> Result<Expr, String> {
        self.level(&["or"], false, Parser::and)
    }

    fn and(&mut self) -> Result<Expr, String> {
        self.level(&["and"], false, Parser::equality)
    }

    fn equality(&mut self) -> Result<Expr, String> {
        self.level(&["==", "!="], true, Parser::comparison)
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        self.level(&["<", "<=", ">", ">="], true, Parser::additive)
    }

    fn additive(&mut self) -> Result<Expr, String> {
        self.level(&["+", "-"], false, Parser::multiplicative)
    }

    fn multiplicative(&mut self) -> Result<Expr, String> {
        self.level(&["*", "/", "%"], false, Parser::unary)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek_op(&["-", "not"]) {
            Some(op) => {
                self.at += 1;
                Ok(Expr::Unary(op, Box::new(self.unary()?)))
            }
            None => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.primary()?;
        loop {
            if self.peek_op(&["."]).is_some() {
                self.at += 1;
                let Some(Token::Name(name)) = self.tokens.get(self.at) else {
                    return Err("a name or a number follows `.`".into());
                };
                let name = name.clone();
                self.at += 1;
                if self.peek_op(&["("]).is_some() {
                    self.at += 1;
                    let mut arguments = Vec::new();
                    while self.peek_op(&[")"]).is_none() {
                        arguments.push(self.or()?);
                        if self.peek_op(&[","]).is_none() {
                            break;
                        }
                        self.at += 1;
                    }
                    self.expect(")")?;
                    expr = Expr::Method(Box::new(expr), name, arguments);
                } else {
                    expr = Expr::Field(Box::new(expr), name);
                }
            } else if self.peek_op(&["["]).is_some() {
                self.at += 1;
                let index = self.or()?;
                self.expect("]")?;
                expr = Expr::Index(Box::new(expr), Box::new(index));
            } else {
                return Ok(expr);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let Some(token) = self.tokens.get(self.at) else { return Err("the expression ends too soon".into()) };
        self.at += 1;
        Ok(Expr::Literal(match token {
            Token::Int(n) => Value::Int(*n),
            Token::Float(x) => Value::Float(*x),
            Token::Text(text) => Value::Text(text.clone()),
            Token::Char(c) => Value::Char(*c),
            Token::Name(name) => match name.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "none" => Value::Shown("none".into()),
                "and" | "or" | "not" => return Err(format!("`{name}` needs something before it")),
                _ => return Ok(Expr::Name(name.clone())),
            },
            Token::Op("(") => {
                let inner = self.or()?;
                self.expect(")")?;
                return Ok(inner);
            }
            Token::Op(op) => return Err(format!("`{op}` does not start an expression")),
        }))
    }
}

/// The names of the bindings an expression reads.
pub(super) fn names(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Literal(_) => {}
        Expr::Name(name) => out.push(name.clone()),
        Expr::Field(inner, _) | Expr::Unary(_, inner) => names(inner, out),
        Expr::Index(inner, index) => {
            names(inner, out);
            names(index, out);
        }
        Expr::Method(inner, _, arguments) => {
            names(inner, out);
            arguments.iter().for_each(|argument| names(argument, out));
        }
        Expr::Binary(_, left, right) => {
            names(left, out);
            names(right, out);
        }
    }
}

/// Whether a name can be a binding's: one that starts with a capital
/// names a type, as in `Role.Admin`.
pub(super) fn binding_name(name: &str) -> bool {
    !name.starts_with(|c: char| c.is_uppercase())
}

// ---------------------------------------------------------------------------
// Evaluating
// ---------------------------------------------------------------------------

/// Evaluates expressions with the bindings `read` shows, each read once.
pub(super) struct Evaluator<'r> {
    read: &'r dyn Fn(&str) -> Result<String, String>,
    texts: HashMap<String, String>,
}

/// What an expression gave: its value, and when it names a binding or a
/// part of one, the path that names it, as `items[3]`.
pub(super) struct Evaluated {
    pub value: Value,
    pub text: String,
    pub path: Option<String>,
}

impl<'r> Evaluator<'r> {
    pub(super) fn new(read: &'r dyn Fn(&str) -> Result<String, String>) -> Self {
        Evaluator { read, texts: HashMap::new() }
    }

    pub(super) fn evaluate(&mut self, expr: &Expr) -> Result<Evaluated, String> {
        if let Some(place) = self.place(expr) {
            let (root, path) = place?;
            let text = self.part(&root, &path)?.to_string();
            return Ok(Evaluated { value: Value::from_shown(&text), text, path: Some(format!("{root}{path}")) });
        }
        let value = self.value(expr)?;
        Ok(Evaluated { text: value.shown(), value, path: None })
    }

    /// Whether a condition holds: an error, or anything but `true`, does not.
    pub(super) fn holds(&mut self, expr: &Expr) -> bool {
        matches!(self.value(expr), Ok(Value::Bool(true)))
    }

    /// The binding and the path a binding's part is at, or `None` for an
    /// expression that computes its value.
    fn place(&mut self, expr: &Expr) -> Option<Result<(String, String), String>> {
        match expr {
            Expr::Name(name) if binding_name(name) => Some(self.text(name).map(|_| (name.clone(), String::new()))),
            Expr::Field(inner, field) => {
                let place = self.place(inner)?;
                Some(place.map(|(root, path)| (root, format!("{path}.{field}"))))
            }
            Expr::Index(inner, index) => {
                let place = self.place(inner)?;
                Some(place.and_then(|(root, path)| {
                    let key = self.value(index)?;
                    Ok((root, format!("{path}[{}]", key.shown())))
                }))
            }
            _ => None,
        }
    }

    fn text(&mut self, name: &str) -> Result<&str, String> {
        if !self.texts.contains_key(name) {
            let text = (self.read)(name)?;
            self.texts.insert(name.to_string(), text);
        }
        Ok(&self.texts[name])
    }

    fn part(&mut self, root: &str, path: &str) -> Result<&str, String> {
        let whole = self.text(root)?;
        shown::find(whole, path).ok_or_else(|| {
            let shown = super::clipped(whole);
            format!("`{root}{path}` is not a part of `{root}`, which is {shown}")
        })
    }

    fn value(&mut self, expr: &Expr) -> Result<Value, String> {
        if let Some(place) = self.place(expr) {
            let (root, path) = place?;
            return Ok(Value::from_shown(self.part(&root, &path)?));
        }
        match expr {
            Expr::Literal(value) => Ok(value.clone()),
            // A variant by its type's name, `Role.Admin`.
            Expr::Name(name) => Ok(Value::Shown(name.clone())),
            Expr::Field(inner, field) => match self.value(inner)? {
                Value::Shown(text)
                    if text.chars().next().is_some_and(char::is_uppercase) && !text.contains(['(', ' ']) =>
                {
                    Ok(Value::Shown(format!("{text}.{field}")))
                }
                other => Err(format!("{} has no `.{field}`", other.kind())),
            },
            Expr::Index(inner, _) => Err(format!("{} has no items", self.value(inner)?.kind())),
            Expr::Method(inner, name, arguments) => {
                let receiver = self.value(inner)?;
                let arguments = arguments.iter().map(|a| self.value(a)).collect::<Result<Vec<_>, _>>()?;
                method(&receiver, name, &arguments)
            }
            Expr::Unary(op, inner) => match (*op, self.value(inner)?) {
                ("not", Value::Bool(b)) => Ok(Value::Bool(!b)),
                ("-", Value::Int(n)) => n.checked_neg().map(Value::Int).ok_or_else(|| "the negation overflows".into()),
                ("-", Value::Float(x)) => Ok(Value::Float(-x)),
                (op, value) => Err(format!("`{op}` does not apply to {}", value.kind())),
            },
            Expr::Binary(op @ ("and" | "or"), left, right) => {
                let truth = |value: Value| match value {
                    Value::Bool(b) => Ok(b),
                    other => Err(format!("`{op}` takes Bools, not {}", other.kind())),
                };
                let left = truth(self.value(left)?)?;
                if left == (*op == "or") {
                    return Ok(Value::Bool(left));
                }
                Ok(Value::Bool(truth(self.value(right)?)?))
            }
            Expr::Binary(op, left, right) => {
                let (left, right) = (self.value(left)?, self.value(right)?);
                binary(op, left, right)
            }
        }
    }
}

fn binary(op: &str, left: Value, right: Value) -> Result<Value, String> {
    use std::cmp::Ordering;
    let mismatch = |left: &Value, right: &Value| format!("`{op}` does not take {} and {}", left.kind(), right.kind());
    if matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
        let order = match (&left, &right) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                float(&left).partial_cmp(&float(&right))
            }
            (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
            (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) if matches!(op, "==" | "!=") => Some(a.cmp(b)),
            // Anything else is equal when it shows the same.
            (Value::Shown(_), _) | (_, Value::Shown(_)) if matches!(op, "==" | "!=") => {
                Some(if left.shown() == right.shown() { Ordering::Equal } else { Ordering::Less })
            }
            _ => return Err(mismatch(&left, &right)),
        };
        return Ok(Value::Bool(match (op, order) {
            ("==", Some(o)) => o.is_eq(),
            ("!=", o) => o.is_none_or(|o| o.is_ne()),
            ("<", Some(o)) => o.is_lt(),
            ("<=", Some(o)) => o.is_le(),
            (">", Some(o)) => o.is_gt(),
            (">=", Some(o)) => o.is_ge(),
            _ => false,
        }));
    }
    match (&left, &right) {
        (Value::Int(a), Value::Int(b)) => {
            let (a, b) = (*a, *b);
            let result = match op {
                "+" => a.checked_add(b),
                "-" => a.checked_sub(b),
                "*" => a.checked_mul(b),
                "/" | "%" if b == 0 => return Err("division by zero".into()),
                "/" => a.checked_div(b),
                _ => a.checked_rem(b),
            };
            result.map(Value::Int).ok_or_else(|| format!("`{op}` overflows"))
        }
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            let (a, b) = (float(&left), float(&right));
            Ok(Value::Float(match op {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" => a / b,
                _ => a % b,
            }))
        }
        (Value::Text(a), Value::Text(b)) if op == "+" => Ok(Value::Text(format!("{a}{b}"))),
        _ => Err(mismatch(&left, &right)),
    }
}

fn float(value: &Value) -> f64 {
    match value {
        Value::Int(n) => *n as f64,
        Value::Float(x) => *x,
        _ => f64::NAN,
    }
}

/// The methods an expression calls: they read a value and change nothing.
fn method(receiver: &Value, name: &str, arguments: &[Value]) -> Result<Value, String> {
    let wrong = || format!("{} has no method `{name}` the debugger evaluates", receiver.kind());
    let arity = |n: usize| match arguments.len() == n {
        true => Ok(()),
        false => Err(format!("`{name}` takes {n} arguments, not {}", arguments.len())),
    };
    match (name, receiver) {
        ("length" | "isEmpty", Value::Text(text)) => {
            arity(0)?;
            let length = text.chars().count();
            Ok(if name == "length" { Value::Int(length as i128) } else { Value::Bool(length == 0) })
        }
        ("length" | "isEmpty", Value::Shown(text)) if collection(text) => {
            arity(0)?;
            let length = shown::parse(text).parts.len();
            Ok(if name == "length" { Value::Int(length as i128) } else { Value::Bool(length == 0) })
        }
        ("isSome" | "isNone", Value::Shown(text)) if text == "none" || text.starts_with("some(") => {
            arity(0)?;
            Ok(Value::Bool((text != "none") == (name == "isSome")))
        }
        ("isOk" | "isErr", Value::Shown(text)) if text.starts_with("ok(") || text.starts_with("err(") => {
            arity(0)?;
            Ok(Value::Bool(text.starts_with("ok(") == (name == "isOk")))
        }
        ("contains" | "startsWith" | "endsWith", Value::Text(text)) => {
            arity(1)?;
            let needle = match &arguments[0] {
                Value::Text(needle) => needle.clone(),
                Value::Char(c) => c.to_string(),
                other => return Err(format!("`{name}` takes a String or a Char, not {}", other.kind())),
            };
            Ok(Value::Bool(match name {
                "contains" => text.contains(&needle),
                "startsWith" => text.starts_with(&needle),
                _ => text.ends_with(&needle),
            }))
        }
        ("contains", Value::Shown(text)) if collection(text) => {
            arity(1)?;
            let wanted = &arguments[0];
            let found = shown::parse(text).parts.iter().any(|(_, part)| {
                let item = Value::from_shown(part.text);
                binary("==", item, wanted.clone()) == Ok(Value::Bool(true))
            });
            Ok(Value::Bool(found))
        }
        _ => Err(wrong()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(text: &str) -> Result<String, String> {
        let read = |name: &str| match name {
            "count" => Ok("3".to_string()),
            "ratio" => Ok("0.5".to_string()),
            "name" => Ok(r#""Ada \"L\"""#.to_string()),
            "items" => Ok(r#"[Item { id: 1, tags: ["a"] }, Item { id: 7, tags: [] }]"#.to_string()),
            "scores" => Ok(r#"["x": 1, "y": 2]"#.to_string()),
            "role" => Ok("Role.Admin".to_string()),
            "found" => Ok("some(2)".to_string()),
            other => Err(format!("no binding `{other}`")),
        };
        let expr = parse(text)?;
        let mut evaluator = Evaluator::new(&read);
        let evaluated = evaluator.evaluate(&expr)?;
        Ok(match evaluated.path {
            Some(path) => format!("{path} = {}", evaluated.text),
            None => evaluated.text,
        })
    }

    #[test]
    fn evaluates_what_a_debugger_reads() {
        assert_eq!(evaluate("count * 2 + 1").as_deref(), Ok("7"));
        assert_eq!(evaluate("count / 2").as_deref(), Ok("1"));
        assert_eq!(evaluate("count + ratio").as_deref(), Ok("3.5"));
        assert_eq!(evaluate("count > 2 and not (ratio >= 1.0)").as_deref(), Ok("true"));
        assert_eq!(evaluate("count == 4 or name == \"Ada \\\"L\\\"\"").as_deref(), Ok("true"));
        assert_eq!(evaluate("items[count - 2].id").as_deref(), Ok("items[1].id = 7"));
        assert_eq!(evaluate("items[0].tags").as_deref(), Ok(r#"items[0].tags = ["a"]"#));
        assert_eq!(evaluate("scores[\"y\"]").as_deref(), Ok(r#"scores["y"] = 2"#));
        assert_eq!(evaluate("items.length() == 2 and items[1].tags.isEmpty()").as_deref(), Ok("true"));
        assert_eq!(evaluate("name.length()").as_deref(), Ok("7"));
        assert_eq!(evaluate("name.startsWith(\"Ada\") and items[0].tags.contains(\"a\")").as_deref(), Ok("true"));
        assert_eq!(evaluate("role == Role.Admin and role != Role.Guest").as_deref(), Ok("true"));
        assert_eq!(evaluate("found.isSome() and found != none").as_deref(), Ok("true"));
        assert_eq!(evaluate("0x10 + 1_000 + 'a'.length()").map_err(|e| e.contains("Char")), Err(true));
        assert_eq!(evaluate("-count % 2").as_deref(), Ok("-1"));
        // Errors say what is wrong.
        assert!(evaluate("count / 0").unwrap_err().contains("division by zero"));
        assert!(evaluate("cont > 1").unwrap_err().contains("no binding `cont`"));
        assert!(evaluate("count && true").unwrap_err().contains("`and`"));
        assert!(evaluate("count == \"3\"").unwrap_err().contains("an Int and a String"));
        assert!(evaluate("items[5]").unwrap_err().contains("not a part"));
        assert!(evaluate("count +").is_err() && evaluate("(count").is_err() && evaluate("").is_err());
    }
}
