//! `tondo bind`: a Tondo module of `extern fn`, `extern type`, aliases and
//! constants from a C header (toolchain 7, language 16.16).
//!
//! The system's C preprocessor (`cc -E -dD`) expands the header; this
//! module reads the declarations and object-like macros of the header and
//! of the headers of its directory it includes, and maps them to Tondo.
//! Types they use from other headers are resolved to what they name; a
//! struct from another header becomes opaque when only pointers reach it.
//! What has no Tondo form (unions by value, bit fields, `long double`,
//! `va_list`, pointer constants) is listed at the end of the module.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

mod dwarf;

/// Writes the module for a library without headers, from its debug
/// information, and returns its text.
pub fn generate_from_library(library: &Path, command: &str) -> Result<String, String> {
    let c = dwarf::library(library)?;
    Ok(Emitter::new(&c).module(command))
}

/// What `tondo bind --package` finds: the package's headers, the
/// preprocessor arguments and the libraries to link, as `[native]
/// libraries` entries; or a library file without headers.
pub struct Package {
    pub headers: Vec<PathBuf>,
    pub arguments: Vec<String>,
    pub libraries: Vec<String>,
    pub library_file: Option<PathBuf>,
}

/// Finds a package through `pkg-config`, then as a library of the system's
/// library paths, whose debug information `generate_from_library` reads.
pub fn find_package(name: &str) -> Result<Package, String> {
    let compiler = std::env::var("TONDO_CC").unwrap_or_else(|_| "cc".into());
    let pkg_config = |args: &[&str]| -> Option<String> {
        let output = Command::new("pkg-config").args(args).arg(name).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let stem = name.strip_prefix("lib").unwrap_or(name);
    let short = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    if let Some(cflags) = pkg_config(&["--cflags"]) {
        let arguments: Vec<String> = cflags.split_whitespace().map(String::from).collect();
        let mut dirs: Vec<PathBuf> = arguments.iter().filter_map(|a| a.strip_prefix("-I")).map(PathBuf::from).collect();
        dirs.extend(pkg_config(&["--variable=includedir"]).map(PathBuf::from));
        let libs = pkg_config(&["--libs"]).unwrap_or_default();
        let search: Vec<PathBuf> =
            libs.split_whitespace().filter_map(|a| a.strip_prefix("-L")).map(PathBuf::from).collect();
        let libraries = libs
            .split_whitespace()
            .filter_map(|a| a.strip_prefix("-l"))
            .map(|library| {
                // A library outside the system's paths is linked by its file.
                let file = search.iter().flat_map(|dir| {
                    ["so", "dylib", "a"].map(|extension| dir.join(format!("lib{library}.{extension}")))
                });
                file.into_iter().find(|f| f.is_file()).map_or(library.to_string(), |f| f.display().to_string())
            })
            .collect();
        let candidates = [name, stem, short];
        let header = dirs.iter().find_map(|dir| {
            candidates.iter().find_map(|candidate| {
                [dir.join(format!("{candidate}.h")), dir.join(candidate).join(format!("{candidate}.h"))]
                    .into_iter()
                    .find(|h| h.is_file())
            })
        });
        if let Some(header) = header {
            return Ok(Package { headers: vec![header], arguments, libraries, library_file: None });
        }
    }
    // A library of the system's paths, bound from its debug information.
    for file in [format!("lib{stem}.so"), format!("lib{stem}.dylib"), format!("lib{stem}.a")] {
        let output = Command::new(&compiler).arg(format!("-print-file-name={file}")).output();
        let path = output.ok().map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
        if let Some(path) = path.filter(|p| p.is_absolute() && p.is_file()) {
            return Ok(Package {
                headers: Vec::new(),
                arguments: Vec::new(),
                libraries: vec![stem.to_string()],
                library_file: Some(path),
            });
        }
    }
    Err(format!(
        "no package `{name}`: pkg-config does not know its headers and the system's library paths have no lib{stem}; pass the header: `tondo bind path/to/{short}.h --library {stem}`"
    ))
}

/// Writes the module for `headers` and returns its text.
pub fn generate(headers: &[PathBuf], arguments: &[String], command: &str) -> Result<String, String> {
    let compiler = std::env::var("TONDO_CC").unwrap_or_else(|_| "cc".into());
    let mut source = String::new();
    for header in headers {
        let header = std::fs::canonicalize(header).map_err(|e| format!("cannot read {}: {e}", header.display()))?;
        source.push_str(&format!("#include \"{}\"\n", header.display()));
    }
    let output = Command::new(&compiler)
        .args(["-E", "-dD", "-x", "c", "-"])
        .args(arguments)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(source.as_bytes())?;
            child.wait_with_output()
        })
        .map_err(|e| format!("cannot run the C preprocessor `{compiler}`: {e}"))?;
    if !output.status.success() {
        return Err(format!("the C preprocessor failed:\n{}", String::from_utf8_lossy(&output.stderr)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let roots: Vec<PathBuf> = headers.iter().filter_map(|h| std::fs::canonicalize(h).ok()).collect();
    let tokens = lex(&text, &roots);
    let mut parser = Parser { tokens: &tokens.tokens, pos: 0, c: C::default() };
    parser.c.macros = tokens.macros;
    parser.program();
    let mut c = parser.c;
    c.targets = tokens.targets;
    Ok(Emitter::new(&c).module(command))
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Number(String),
    Char(i128),
    Str(String),
    Punct(&'static str),
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    /// The file it comes from, an index into `Lexed::files`.
    file: usize,
}

struct Lexed {
    tokens: Vec<Token>,
    /// Object-like macros by name: their file and body.
    macros: Vec<Macro>,
    /// The files whose declarations the module binds.
    targets: HashSet<usize>,
}

#[derive(Clone)]
struct Macro {
    name: String,
    file: usize,
    /// The parameters of a function-like macro.
    params: Option<Vec<String>>,
    body: Vec<Token>,
}

const PUNCTUATORS: [&str; 47] = [
    "...", "<<=", ">>=", "->", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "*=", "/=", "%=", "+=",
    "-=", "&=", "^=", "|=", "##", "[", "]", "(", ")", "{", "}", ".", "&", "*", "+", "-", "~", "!", "/", "%", "<", ">",
    "^", "|", "?", ":", ";", ",", "=",
];

/// The tokens of preprocessed C, with the file of each from the line
/// markers, and the object-like macros `-dD` leaves in place.
fn lex(text: &str, roots: &[PathBuf]) -> Lexed {
    let mut files: Vec<PathBuf> = Vec::new();
    let file_index = |files: &mut Vec<PathBuf>, path: &str| {
        let path = PathBuf::from(path);
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        match files.iter().position(|f| *f == path) {
            Some(index) => index,
            None => {
                files.push(path);
                files.len() - 1
            }
        }
    };
    let mut current = file_index(&mut files, "<stdin>");
    let mut targets: HashSet<usize> = HashSet::new();
    let mut tokens = Vec::new();
    let mut macros = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim_start();
            if let Some(name_and_body) = rest.strip_prefix("define ") {
                let name: String =
                    name_and_body.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                let after = &name_and_body[name.len()..];
                if name.is_empty() {
                    continue;
                }
                // A function-like macro has `(` right after its name.
                let (params, body) = match after.strip_prefix('(').and_then(|rest| rest.split_once(')')) {
                    Some((params, body)) => {
                        let params: Vec<String> = params.split(',').map(|p| p.trim().to_string()).collect();
                        let params = params.into_iter().filter(|p| !p.is_empty()).collect();
                        (Some(params), body)
                    }
                    None => (None, after),
                };
                macros.push(Macro { name, file: current, params, body: scan(body, current) });
                continue;
            }
            // A line marker: `# 12 "file" 1 3`.
            let mut parts = rest.split_whitespace();
            if parts.next().is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
                && let Some(quoted) = rest.split('"').nth(1)
            {
                let flags: Vec<&str> = rest.rsplit('"').next().unwrap_or("").split_whitespace().collect();
                let next = file_index(&mut files, quoted);
                // A header of a bound header's directory that it includes
                // directly is bound too.
                let entering = flags.first() == Some(&"1");
                let root = roots.contains(&files[next]);
                let sibling = entering
                    && roots.contains(&files[current])
                    && files[next].parent().is_some()
                    && files[next].parent() == files[current].parent();
                if root || sibling {
                    targets.insert(next);
                }
                current = next;
            }
            continue;
        }
        tokens.extend(scan(line, current));
    }
    Lexed { tokens, macros, targets }
}

/// The tokens of one line of C.
fn scan(line: &str, file: usize) -> Vec<Token> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            break;
        }
        let start = i;
        let tok = if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // A prefixed string or character literal: `L"..."`, `u8"..."`.
            if matches!(word.as_str(), "L" | "u" | "U" | "u8") && matches!(chars.get(i), Some('"' | '\'')) {
                continue;
            }
            Tok::Ident(word)
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            while i < chars.len() {
                let d = chars[i];
                let exponent = matches!(d, '+' | '-') && matches!(chars[i - 1], 'e' | 'E' | 'p' | 'P');
                if d.is_ascii_alphanumeric() || d == '.' || d == '_' || exponent {
                    i += 1;
                } else {
                    break;
                }
            }
            Tok::Number(chars[start..i].iter().collect())
        } else if c == '"' || c == '\'' {
            i += 1;
            let mut text = String::new();
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    text.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        other => other,
                    });
                } else {
                    text.push(chars[i]);
                }
                i += 1;
            }
            i += 1;
            if c == '"' { Tok::Str(text) } else { Tok::Char(text.chars().next().map_or(0, |c| c as i128)) }
        } else {
            let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
            match PUNCTUATORS.iter().find(|p| rest.starts_with(**p)) {
                Some(p) => {
                    i += p.len();
                    Tok::Punct(p)
                }
                None => {
                    i += 1;
                    continue;
                }
            }
        };
        out.push(Token { tok, file });
    }
    out
}

// ---------------------------------------------------------------------------
// C declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum CType {
    Void,
    Bool,
    Char,
    SChar,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Long,
    ULong,
    LongLong,
    ULongLong,
    Float,
    Double,
    /// A type Tondo cannot represent, by its C spelling.
    Unsupported(&'static str),
    Named(String),
    Record(usize),
    Enum(usize),
    /// A pointer, and whether what it points to is `const`.
    Pointer(Box<CType>, bool),
    Array(Box<CType>, Option<u64>),
    Function(Box<CFunction>),
    /// The place of the declared name while a declarator is read.
    Hole,
}

/// Parameters, each with its name if it has one.
type Params = Vec<(Option<String>, CType)>;

#[derive(Clone, Debug, PartialEq)]
struct CFunction {
    result: CType,
    params: Params,
    variadic: bool,
}

struct Record {
    tag: Option<String>,
    union: bool,
    fields: Option<Vec<CField>>,
    file: usize,
}

struct CField {
    name: Option<String>,
    ty: CType,
    /// The width of a bit field.
    bits: Option<u64>,
}

struct CEnum {
    tag: Option<String>,
    items: Vec<(String, Option<i128>)>,
    file: usize,
}

struct Typedef {
    name: String,
    ty: CType,
    file: usize,
}

struct Function {
    name: String,
    ty: CFunction,
    /// The symbol an `__asm__` label gives, when it differs.
    symbol: Option<String>,
    file: usize,
    inline: bool,
}

#[derive(Default)]
struct C {
    records: Vec<Record>,
    enums: Vec<CEnum>,
    typedefs: Vec<Typedef>,
    typedef_names: HashMap<String, usize>,
    functions: Vec<Function>,
    macros: Vec<Macro>,
    /// Enum constants by name, with their values.
    constants: HashMap<String, i128>,
    targets: HashSet<usize>,
    /// The declarations in source order: what kind, and its index.
    order: Vec<(Kind, usize)>,
    /// Whether a struct a pointer reaches is bound whole, as for a library
    /// read from its debug information, whose structs are its own.
    complete_records: bool,
}

#[derive(Copy, Clone, PartialEq)]
enum Kind {
    Record,
    Enum,
    Typedef,
    Function,
}

type PResult<T> = Result<T, ()>;

struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
    c: C,
}

/// What declaration specifiers say.
#[derive(Default)]
struct Specifiers {
    ty: Option<CType>,
    words: Vec<String>,
    is_const: bool,
    typedef: bool,
    inline: bool,
    file: usize,
}

const QUALIFIERS: [&str; 16] = [
    "const",
    "volatile",
    "restrict",
    "__restrict",
    "__restrict__",
    "_Atomic",
    "__const",
    "__volatile__",
    "_Nullable",
    "_Nonnull",
    "_Null_unspecified",
    "__nullable",
    "__nonnull",
    "__extension__",
    "_Noreturn",
    "__unaligned",
];

const STORAGE: [&str; 9] =
    ["extern", "static", "auto", "register", "_Thread_local", "__thread", "inline", "__inline", "__inline__"];

const BASIC: [&str; 12] = [
    "void",
    "char",
    "short",
    "int",
    "long",
    "float",
    "double",
    "signed",
    "unsigned",
    "_Bool",
    "__signed__",
    "__int128",
];

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.tokens.get(self.pos + n).map(|t| &t.tok)
    }

    fn at(&self, punct: &str) -> bool {
        matches!(self.peek(), Some(Tok::Punct(p)) if *p == punct)
    }

    fn eat(&mut self, punct: &str) -> bool {
        if self.at(punct) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, punct: &str) -> PResult<()> {
        if self.eat(punct) { Ok(()) } else { Err(()) }
    }

    fn word(&self) -> Option<&str> {
        match self.peek() {
            Some(Tok::Ident(word)) => Some(word),
            _ => None,
        }
    }

    fn file(&self) -> usize {
        self.tokens.get(self.pos).map_or(0, |t| t.file)
    }

    /// Skips a balanced group that starts at the current `(`, `[` or `{`.
    fn skip_group(&mut self) {
        let mut depth = 0i32;
        while let Some(tok) = self.peek() {
            match tok {
                Tok::Punct("(" | "[" | "{") => depth += 1,
                Tok::Punct(")" | "]" | "}") => depth -= 1,
                _ => {}
            }
            self.pos += 1;
            if depth <= 0 {
                return;
            }
        }
    }

    /// Skips GNU and Microsoft attributes; returns the label of `__asm__`.
    fn attributes(&mut self) -> Option<String> {
        let mut label = None;
        loop {
            match self.word() {
                Some("__attribute__" | "__attribute" | "__declspec" | "_Alignas" | "__alignas") => {
                    self.pos += 1;
                    self.skip_group();
                }
                Some("__asm__" | "__asm" | "asm") => {
                    self.pos += 1;
                    let start = self.pos;
                    self.skip_group();
                    let text: String = self.tokens[start..self.pos]
                        .iter()
                        .filter_map(|t| match &t.tok {
                            Tok::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .collect();
                    label = Some(text);
                }
                _ => return label,
            }
        }
    }

    fn program(&mut self) {
        while self.pos < self.tokens.len() {
            let start = self.pos;
            if self.eat(";") {
                continue;
            }
            if matches!(self.word(), Some("_Static_assert" | "static_assert")) {
                self.recover();
                continue;
            }
            if self.declaration().is_err() {
                self.pos = start.max(self.pos);
                self.recover();
            }
        }
    }

    /// Skips to after the `;` that ends a declaration at depth 0.
    fn recover(&mut self) {
        while let Some(tok) = self.peek() {
            match tok {
                Tok::Punct(";") => {
                    self.pos += 1;
                    return;
                }
                Tok::Punct("(" | "[" | "{") => self.skip_group(),
                _ => self.pos += 1,
            }
        }
    }

    fn declaration(&mut self) -> PResult<()> {
        let specifiers = self.specifiers()?;
        let base = specifiers.ty.clone().unwrap_or(CType::Int);
        if self.eat(";") {
            return Ok(());
        }
        loop {
            let (name, ty, label) = self.declarator(base.clone(), specifiers.is_const)?;
            self.attributes();
            if let Some(name) = name {
                if specifiers.typedef {
                    self.c.typedefs.push(Typedef { name: name.clone(), ty, file: specifiers.file });
                    self.c.typedef_names.insert(name, self.c.typedefs.len() - 1);
                    self.c.order.push((Kind::Typedef, self.c.typedefs.len() - 1));
                } else if let CType::Function(function) = ty {
                    let symbol = label.filter(|l| *l != name);
                    self.c.functions.push(Function {
                        name,
                        ty: *function,
                        symbol,
                        file: specifiers.file,
                        inline: specifiers.inline,
                    });
                    self.c.order.push((Kind::Function, self.c.functions.len() - 1));
                }
            }
            if self.at("{") {
                // A function's body.
                self.skip_group();
                return Ok(());
            }
            if self.eat("=") {
                while !self.at(",") && !self.at(";") && self.peek().is_some() {
                    if matches!(self.peek(), Some(Tok::Punct("(" | "[" | "{"))) {
                        self.skip_group();
                    } else {
                        self.pos += 1;
                    }
                }
            }
            if self.eat(";") {
                return Ok(());
            }
            self.expect(",")?;
        }
    }

    fn is_type_start(&self) -> bool {
        match self.word() {
            Some(word) => {
                BASIC.contains(&word)
                    || QUALIFIERS.contains(&word)
                    || STORAGE.contains(&word)
                    || matches!(word, "struct" | "union" | "enum" | "typedef" | "__attribute__" | "_Bool" | "bool")
                    || self.c.typedef_names.contains_key(word)
                    || word == "__builtin_va_list"
            }
            None => false,
        }
    }

    fn specifiers(&mut self) -> PResult<Specifiers> {
        let mut s = Specifiers { file: self.file(), ..Specifiers::default() };
        loop {
            self.attributes();
            let Some(word) = self.word().map(str::to_string) else { break };
            match word.as_str() {
                "typedef" => s.typedef = true,
                "inline" | "__inline" | "__inline__" => s.inline = true,
                w if STORAGE.contains(&w) => {}
                "const" | "__const" => s.is_const = true,
                w if QUALIFIERS.contains(&w) => {
                    if w == "_Atomic" && matches!(self.peek_at(1), Some(Tok::Punct("("))) {
                        self.pos += 1;
                        self.skip_group();
                        s.ty = Some(CType::Unsupported("_Atomic"));
                        continue;
                    }
                }
                w if BASIC.contains(&w) || w == "bool" => s.words.push(word),
                "struct" | "union" => {
                    self.pos += 1;
                    s.ty = Some(self.record(word == "union")?);
                    continue;
                }
                "enum" => {
                    self.pos += 1;
                    s.ty = Some(self.enumeration()?);
                    continue;
                }
                "__builtin_va_list" => s.ty = Some(CType::Unsupported("va_list")),
                "__typeof__" | "typeof" | "__typeof" => {
                    self.pos += 1;
                    self.skip_group();
                    s.ty = Some(CType::Unsupported("typeof"));
                    continue;
                }
                w if s.ty.is_none() && s.words.is_empty() && self.c.typedef_names.contains_key(w) => {
                    s.ty = Some(CType::Named(word));
                }
                _ => break,
            }
            self.pos += 1;
        }
        if !s.words.is_empty() {
            s.ty = Some(basic(&s.words));
        }
        if s.ty.is_none() && !s.typedef && s.words.is_empty() && !s.inline {
            // Not a declaration this module understands.
            if !self.is_type_start() && !matches!(self.peek(), Some(Tok::Ident(_))) {
                return Err(());
            }
        }
        Ok(s)
    }

    fn record(&mut self, union: bool) -> PResult<CType> {
        self.attributes();
        let file = self.file();
        let tag = self.word().filter(|w| !matches!(*w, "{")).map(str::to_string);
        if tag.is_some() {
            self.pos += 1;
        }
        self.attributes();
        let existing = tag
            .as_ref()
            .and_then(|t| self.c.records.iter().position(|r| r.tag.as_ref() == Some(t) && r.union == union));
        let id = match existing {
            Some(id) => id,
            None => {
                self.c.records.push(Record { tag: tag.clone(), union, fields: None, file });
                self.c.order.push((Kind::Record, self.c.records.len() - 1));
                self.c.records.len() - 1
            }
        };
        if self.eat("{") {
            let mut fields = Vec::new();
            while !self.eat("}") {
                if self.peek().is_none() {
                    return Err(());
                }
                if self.eat(";") {
                    continue;
                }
                let specifiers = self.specifiers()?;
                let base = specifiers.ty.clone().unwrap_or(CType::Int);
                if self.eat(";") {
                    // An anonymous struct or union member.
                    fields.push(CField { name: None, ty: base, bits: None });
                    continue;
                }
                loop {
                    let (name, ty, _) = self.declarator(base.clone(), specifiers.is_const)?;
                    let bits = if self.eat(":") {
                        let start = self.pos;
                        while !self.at(",") && !self.at(";") && self.peek().is_some() {
                            self.pos += 1;
                        }
                        Some(Evaluator::new(&self.c).integer(&self.tokens[start..self.pos]).unwrap_or(0) as u64)
                    } else {
                        None
                    };
                    self.attributes();
                    fields.push(CField { name, ty, bits });
                    if self.eat(";") {
                        break;
                    }
                    self.expect(",")?;
                }
            }
            self.attributes();
            let record = &mut self.c.records[id];
            record.fields = Some(fields);
            record.file = file;
        }
        Ok(CType::Record(id))
    }

    fn enumeration(&mut self) -> PResult<CType> {
        self.attributes();
        let file = self.file();
        let tag = self.word().map(str::to_string);
        if tag.is_some() {
            self.pos += 1;
        }
        // `enum Name : Type` of C23.
        if self.eat(":") {
            self.specifiers()?;
        }
        let existing = tag.as_ref().and_then(|t| self.c.enums.iter().position(|e| e.tag.as_ref() == Some(t)));
        let id = match existing {
            Some(id) => id,
            None => {
                self.c.enums.push(CEnum { tag: tag.clone(), items: Vec::new(), file });
                self.c.order.push((Kind::Enum, self.c.enums.len() - 1));
                self.c.enums.len() - 1
            }
        };
        if self.eat("{") {
            let mut next = 0i128;
            let mut items = Vec::new();
            while !self.eat("}") {
                let Some(name) = self.word().map(str::to_string) else { return Err(()) };
                self.pos += 1;
                self.attributes();
                let value = if self.eat("=") {
                    let start = self.pos;
                    while !self.at(",") && !self.at("}") && self.peek().is_some() {
                        if self.at("(") {
                            self.skip_group();
                        } else {
                            self.pos += 1;
                        }
                    }
                    let expression = self.tokens[start..self.pos].to_vec();
                    Evaluator::new(&self.c).integer(&expression)
                } else {
                    Some(next)
                };
                if let Some(value) = value {
                    self.c.constants.insert(name.clone(), value);
                    next = value + 1;
                }
                items.push((name, value));
                self.eat(",");
            }
            self.c.enums[id].items = items;
            self.c.enums[id].file = file;
        }
        Ok(CType::Enum(id))
    }

    /// A declarator over `base`: the declared name, its type and the
    /// label of `__asm__` after it.
    fn declarator(&mut self, base: CType, base_const: bool) -> PResult<(Option<String>, CType, Option<String>)> {
        self.attributes();
        let mut ty = base;
        let mut pointee_const = base_const;
        while self.eat("*") || self.eat("^") {
            ty = CType::Pointer(Box::new(ty), pointee_const);
            pointee_const = false;
            while let Some(word) = self.word() {
                if QUALIFIERS.contains(&word) {
                    pointee_const = matches!(word, "const" | "__const");
                    self.pos += 1;
                } else if matches!(word, "__attribute__" | "__attribute") {
                    self.attributes();
                } else {
                    break;
                }
            }
        }
        let mut name = None;
        let mut inner = None;
        if let Some(word) = self.word()
            && !self.c.typedef_names.contains_key(word)
            && !BASIC.contains(&word)
            && !QUALIFIERS.contains(&word)
            && !matches!(word, "__attribute__" | "__asm__" | "__asm")
        {
            name = Some(word.to_string());
            self.pos += 1;
        } else if self.at("(") && self.nested_declarator() {
            self.pos += 1;
            inner = Some(self.declarator(CType::Hole, false)?);
            self.expect(")")?;
        }
        // Suffixes apply from the last to the first.
        let mut suffixes = Vec::new();
        loop {
            if self.eat("[") {
                let start = self.pos;
                while !self.at("]") && self.peek().is_some() {
                    self.pos += 1;
                }
                let size = Evaluator::new(&self.c).integer(&self.tokens[start..self.pos]).map(|n| n as u64);
                self.expect("]")?;
                suffixes.push(Suffix::Array(size));
            } else if self.eat("(") {
                suffixes.push(Suffix::Function(self.parameters()?));
            } else {
                break;
            }
        }
        for suffix in suffixes.into_iter().rev() {
            ty = match suffix {
                Suffix::Array(size) => CType::Array(Box::new(ty), size),
                Suffix::Function((params, variadic)) => {
                    CType::Function(Box::new(CFunction { result: ty, params, variadic }))
                }
            };
        }
        let label = self.attributes();
        Ok(match inner {
            Some((inner_name, inner_ty, inner_label)) => (inner_name, fill(inner_ty, &ty), inner_label.or(label)),
            None => (name, ty, label),
        })
    }

    /// Whether the `(` at the current token opens a nested declarator, as
    /// in `(*callback)(int)`, rather than parameters.
    fn nested_declarator(&self) -> bool {
        match self.peek_at(1) {
            Some(Tok::Punct("*" | "^" | "(")) => true,
            Some(Tok::Ident(word)) => {
                !self.c.typedef_names.contains_key(word)
                    && !BASIC.contains(&word.as_str())
                    && !QUALIFIERS.contains(&word.as_str())
                    && !matches!(word.as_str(), "struct" | "union" | "enum" | "bool")
                    && !word.starts_with("__attribute")
            }
            _ => false,
        }
    }

    /// Parameters after `(`, up to and including `)`.
    fn parameters(&mut self) -> PResult<(Params, bool)> {
        let mut params = Vec::new();
        let mut variadic = false;
        if self.eat(")") {
            return Ok((params, false));
        }
        if self.word() == Some("void") && matches!(self.peek_at(1), Some(Tok::Punct(")"))) {
            self.pos += 2;
            return Ok((params, false));
        }
        loop {
            if self.eat("...") {
                variadic = true;
                self.expect(")")?;
                return Ok((params, variadic));
            }
            let specifiers = self.specifiers()?;
            let base = specifiers.ty.clone().unwrap_or(CType::Int);
            let (name, ty, _) = self.declarator(base, specifiers.is_const)?;
            // Arrays and functions as parameters are pointers.
            let ty = match ty {
                CType::Array(element, _) => CType::Pointer(element, specifiers.is_const),
                CType::Function(_) => CType::Pointer(Box::new(ty), false),
                ty => ty,
            };
            params.push((name, ty));
            if self.eat(")") {
                return Ok((params, variadic));
            }
            self.expect(",")?;
        }
    }
}

enum Suffix {
    Array(Option<u64>),
    Function((Params, bool)),
}

/// `ty` with its hole replaced by `with`.
fn fill(ty: CType, with: &CType) -> CType {
    match ty {
        CType::Hole => with.clone(),
        CType::Pointer(inner, c) => CType::Pointer(Box::new(fill(*inner, with)), c),
        CType::Array(inner, size) => CType::Array(Box::new(fill(*inner, with)), size),
        CType::Function(f) => {
            let CFunction { result, params, variadic } = *f;
            CType::Function(Box::new(CFunction { result: fill(result, with), params, variadic }))
        }
        other => other,
    }
}

/// The type the words of a basic type spell, such as `unsigned long int`.
fn basic(words: &[String]) -> CType {
    let count = |w: &str| words.iter().filter(|x| *x == w).count();
    let unsigned = count("unsigned") > 0;
    let signed = count("signed") + count("__signed__") > 0;
    if count("void") > 0 {
        CType::Void
    } else if count("_Bool") + count("bool") > 0 {
        CType::Bool
    } else if count("__int128") > 0 {
        CType::Unsupported("__int128")
    } else if count("char") > 0 {
        if unsigned {
            CType::UChar
        } else if signed {
            CType::SChar
        } else {
            CType::Char
        }
    } else if count("short") > 0 {
        if unsigned { CType::UShort } else { CType::Short }
    } else if count("float") > 0 {
        CType::Float
    } else if count("double") > 0 {
        if count("long") > 0 { CType::Unsupported("long double") } else { CType::Double }
    } else if count("long") >= 2 {
        if unsigned { CType::ULongLong } else { CType::LongLong }
    } else if count("long") == 1 {
        if unsigned { CType::ULong } else { CType::Long }
    } else if unsigned {
        CType::UInt
    } else {
        CType::Int
    }
}

// ---------------------------------------------------------------------------
// Constant expressions
// ---------------------------------------------------------------------------

/// A C constant: an integer of a C type, a floating number or text.
#[derive(Clone, Debug)]
enum Value {
    Int(i128, CType),
    Float(f64, bool),
    Text(String),
}

struct Evaluator<'c> {
    c: &'c C,
    tokens: Vec<Token>,
    pos: usize,
    /// Macros being expanded, against cycles.
    expanding: Vec<String>,
}

impl<'c> Evaluator<'c> {
    fn new(c: &'c C) -> Evaluator<'c> {
        Evaluator { c, tokens: Vec::new(), pos: 0, expanding: Vec::new() }
    }

    fn integer(&mut self, tokens: &[Token]) -> Option<i128> {
        match self.value(tokens)? {
            Value::Int(n, _) => Some(n),
            _ => None,
        }
    }

    fn value(&mut self, tokens: &[Token]) -> Option<Value> {
        if tokens.is_empty() {
            return None;
        }
        let saved = (std::mem::replace(&mut self.tokens, tokens.to_vec()), self.pos);
        self.pos = 0;
        let value = self.binary(0);
        let complete = self.pos == self.tokens.len();
        (self.tokens, self.pos) = saved;
        value.filter(|_| complete)
    }

    fn punct(&self) -> Option<&'static str> {
        match self.tokens.get(self.pos).map(|t| &t.tok) {
            Some(Tok::Punct(p)) => Some(p),
            _ => None,
        }
    }

    fn binary(&mut self, min: u8) -> Option<Value> {
        let mut left = self.unary()?;
        loop {
            let Some(op) = self.punct() else { return Some(left) };
            let precedence = match op {
                "||" => 1,
                "&&" => 2,
                "|" => 3,
                "^" => 4,
                "&" => 5,
                "==" | "!=" => 6,
                "<" | ">" | "<=" | ">=" => 7,
                "<<" | ">>" => 8,
                "+" | "-" => 9,
                "*" | "/" | "%" => 10,
                _ => return Some(left),
            };
            if precedence <= min {
                return Some(left);
            }
            self.pos += 1;
            let right = self.binary(precedence)?;
            left = arithmetic(op, left, right)?;
        }
    }

    fn unary(&mut self) -> Option<Value> {
        match self.punct() {
            Some("-") => {
                self.pos += 1;
                match self.unary()? {
                    Value::Int(n, ty) => Some(Value::Int(-n, ty)),
                    Value::Float(f, single) => Some(Value::Float(-f, single)),
                    Value::Text(_) => None,
                }
            }
            Some("+") => {
                self.pos += 1;
                self.unary()
            }
            Some("~") => {
                self.pos += 1;
                match self.unary()? {
                    Value::Int(n, ty) => Some(Value::Int(wrap(!n, &ty), ty)),
                    _ => None,
                }
            }
            Some("!") => {
                self.pos += 1;
                match self.unary()? {
                    Value::Int(n, _) => Some(Value::Int((n == 0) as i128, CType::Int)),
                    _ => None,
                }
            }
            Some("(") => {
                self.pos += 1;
                // A cast to an integer type: `(int)5`, `(uint32_t)1`.
                if let Some(ty) = self.cast_type() {
                    let value = self.unary()?;
                    return match (value, integer_kind(self.c, &ty)) {
                        (Value::Int(n, _), Some(kind)) => Some(Value::Int(wrap(n, &kind), kind)),
                        (Value::Float(f, _), _) if matches!(ty, CType::Double) => Some(Value::Float(f, false)),
                        _ => None,
                    };
                }
                let value = self.binary(0)?;
                if self.punct() != Some(")") {
                    return None;
                }
                self.pos += 1;
                Some(value)
            }
            _ => self.primary(),
        }
    }

    /// The type of a cast at the current position, after its `(`.
    fn cast_type(&mut self) -> Option<CType> {
        let start = self.pos;
        let mut words = Vec::new();
        while let Some(Tok::Ident(word)) = self.tokens.get(self.pos).map(|t| &t.tok) {
            if BASIC.contains(&word.as_str()) || word == "bool" || QUALIFIERS.contains(&word.as_str()) {
                words.push(word.clone());
                self.pos += 1;
            } else if words.is_empty() && self.c.typedef_names.contains_key(word) {
                let ty = CType::Named(word.clone());
                self.pos += 1;
                if self.punct() == Some(")") {
                    self.pos += 1;
                    return Some(ty);
                }
                // A pointer cast, which a constant cannot hold.
                self.pos = start;
                return None;
            } else {
                break;
            }
        }
        let words: Vec<String> = words.into_iter().filter(|w| !QUALIFIERS.contains(&w.as_str())).collect();
        if words.is_empty() || self.punct() != Some(")") {
            self.pos = start;
            return None;
        }
        self.pos += 1;
        Some(basic(&words))
    }

    fn primary(&mut self) -> Option<Value> {
        let token = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        match token.tok {
            Tok::Number(text) => number(&text),
            Tok::Char(c) => Some(Value::Int(c, CType::Int)),
            Tok::Str(mut text) => {
                // Adjacent strings join.
                while let Some(Tok::Str(more)) = self.tokens.get(self.pos).map(|t| &t.tok) {
                    text.push_str(more);
                    self.pos += 1;
                }
                Some(Value::Text(text))
            }
            Tok::Ident(name) if name == "sizeof" => {
                // `sizeof(type)`, as C lays the type out.
                if self.punct() != Some("(") {
                    return None;
                }
                let start = self.pos;
                let mut depth = 0;
                while let Some(token) = self.tokens.get(self.pos) {
                    match token.tok {
                        Tok::Punct("(") => depth += 1,
                        Tok::Punct(")") => depth -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                    if depth == 0 {
                        break;
                    }
                }
                let ty = type_of(self.c, &self.tokens[start + 1..self.pos - 1])?;
                Some(Value::Int(layout(self.c, &ty).0 as i128, CType::ULong))
            }
            Tok::Ident(name) => {
                if let Some(&value) = self.c.constants.get(&name) {
                    return Some(Value::Int(value, CType::Int));
                }
                if self.expanding.contains(&name) {
                    return None;
                }
                let body = self.c.macros.iter().rev().find(|m| m.name == name && m.params.is_none())?.body.clone();
                self.expanding.push(name);
                let value = self.value(&body);
                self.expanding.pop();
                value
            }
            Tok::Punct(_) => None,
        }
    }
}

/// The type that C tokens spell, as in a cast or `sizeof`: words of a basic
/// type, a typedef name or a tagged type, then pointers.
fn type_of(c: &C, tokens: &[Token]) -> Option<CType> {
    let words: Vec<&str> = tokens
        .iter()
        .filter_map(|t| match &t.tok {
            Tok::Ident(w) if !QUALIFIERS.contains(&w.as_str()) => Some(w.as_str()),
            _ => None,
        })
        .collect();
    let pointers = tokens.iter().filter(|t| t.tok == Tok::Punct("*")).count();
    if tokens.iter().any(|t| matches!(t.tok, Tok::Punct(p) if p != "*")) {
        return None;
    }
    let mut ty = match words.as_slice() {
        ["struct" | "union", tag] => CType::Record(c.records.iter().position(|r| r.tag.as_deref() == Some(*tag))?),
        ["enum", _] => CType::Int,
        [name] if c.typedef_names.contains_key(*name) => CType::Named(name.to_string()),
        words if !words.is_empty() && words.iter().all(|w| BASIC.contains(w) || *w == "bool") => {
            basic(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
        }
        _ => return None,
    };
    for _ in 0..pointers {
        ty = CType::Pointer(Box::new(ty), false);
    }
    Some(ty)
}

/// `body` without the parentheses around the whole of it.
fn unwrap(mut body: &[Token]) -> &[Token] {
    while body.len() >= 2 && body[0].tok == Tok::Punct("(") && body[body.len() - 1].tok == Tok::Punct(")") {
        // The first `(` must close at the end.
        let mut depth = 0;
        let closes_at_end = body.iter().enumerate().all(|(i, t)| {
            match t.tok {
                Tok::Punct("(") => depth += 1,
                Tok::Punct(")") => depth -= 1,
                _ => {}
            }
            depth > 0 || i == body.len() - 1
        });
        if !closes_at_end {
            break;
        }
        body = &body[1..body.len() - 1];
    }
    body
}

/// `( type ) value`: a cast, its type and what follows it.
fn cast(c: &C, body: &[Token]) -> Option<(CType, Vec<Token>)> {
    let body = unwrap(body);
    if body.first()?.tok != Tok::Punct("(") {
        return None;
    }
    let close = body.iter().position(|t| t.tok == Tok::Punct(")"))?;
    let ty = type_of(c, &body[1..close])?;
    Some((ty, body[close + 1..].to_vec()))
}

/// The arguments of a call `name(a, b)`, split at the commas outside
/// parentheses.
fn call(body: &[Token]) -> Option<(String, Vec<Vec<Token>>)> {
    let body = unwrap(body);
    let Tok::Ident(name) = &body.first()?.tok else { return None };
    if body.get(1)?.tok != Tok::Punct("(") || body.last()?.tok != Tok::Punct(")") {
        return None;
    }
    let (mut args, mut current, mut depth) = (Vec::new(), Vec::new(), 0);
    for token in &body[2..body.len() - 1] {
        match token.tok {
            Tok::Punct("(") => depth += 1,
            Tok::Punct(")") => depth -= 1,
            Tok::Punct(",") if depth == 0 => {
                args.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
        current.push(token.clone());
    }
    if !current.is_empty() {
        args.push(current);
    }
    Some((name.clone(), args))
}

/// The value of a C number literal, typed as C types it.
fn number(text: &str) -> Option<Value> {
    let lower = text.to_ascii_lowercase();
    let hex = lower.starts_with("0x");
    let float = !hex && (lower.contains('.') || lower.contains('e')) || hex && lower.contains('p');
    if float {
        let single = lower.ends_with('f');
        let digits = lower.trim_end_matches(['f', 'l']);
        return digits.parse::<f64>().ok().map(|f| Value::Float(f, single));
    }
    let digits = lower.trim_end_matches(['u', 'l']);
    let suffix = &lower[digits.len()..];
    let value = if hex {
        i128::from_str_radix(&digits[2..], 16).ok()?
    } else if let Some(binary) = digits.strip_prefix("0b") {
        i128::from_str_radix(binary, 2).ok()?
    } else if digits.len() > 1 && digits.starts_with('0') {
        i128::from_str_radix(&digits[1..], 8).ok()?
    } else {
        digits.parse().ok()?
    };
    let unsigned = suffix.contains('u');
    let longs = suffix.matches('l').count();
    // The first type that holds the value (C 6.4.4.1); octal and hex may
    // take an unsigned type.
    let candidates: &[CType] = match (unsigned, longs) {
        (true, 0) => &[CType::UInt, CType::ULong],
        (true, 1) => &[CType::ULong],
        (true, _) => &[CType::ULongLong],
        (false, 0) if hex || digits.starts_with('0') => &[CType::Int, CType::UInt, CType::Long, CType::ULong],
        (false, 0) => &[CType::Int, CType::Long],
        (false, 1) if hex => &[CType::Long, CType::ULong],
        (false, 1) => &[CType::Long],
        (false, _) => &[CType::LongLong, CType::ULongLong],
    };
    let ty = candidates.iter().find(|t| fits(value, t)).cloned().unwrap_or(CType::ULongLong);
    Some(Value::Int(value, ty))
}

fn bits_and_sign(ty: &CType) -> (u32, bool) {
    match ty {
        CType::Bool | CType::UChar => (8, false),
        CType::Char | CType::SChar => (8, true),
        CType::Short => (16, true),
        CType::UShort => (16, false),
        CType::Int => (32, true),
        CType::UInt => (32, false),
        CType::Long | CType::LongLong => (64, true),
        _ => (64, false),
    }
}

fn fits(value: i128, ty: &CType) -> bool {
    let (bits, signed) = bits_and_sign(ty);
    if signed {
        value >= -(1i128 << (bits - 1)) && value < (1i128 << (bits - 1))
    } else {
        value >= 0 && value < (1i128 << bits)
    }
}

/// `value` wrapped to the C integer type, as C converts.
fn wrap(value: i128, ty: &CType) -> i128 {
    let (bits, signed) = bits_and_sign(ty);
    let masked = value & ((1i128 << bits) - 1);
    if signed && masked >= 1i128 << (bits - 1) { masked - (1i128 << bits) } else { masked }
}

/// The integer type a C type names, through typedefs.
fn integer_kind(c: &C, ty: &CType) -> Option<CType> {
    match ty {
        CType::Named(name) => integer_kind(c, &c.typedefs[*c.typedef_names.get(name)?].ty),
        CType::Enum(_) => Some(CType::Int),
        CType::Float | CType::Double | CType::Void => None,
        CType::Bool
        | CType::Char
        | CType::SChar
        | CType::UChar
        | CType::Short
        | CType::UShort
        | CType::Int
        | CType::UInt
        | CType::Long
        | CType::ULong
        | CType::LongLong
        | CType::ULongLong => Some(ty.clone()),
        _ => None,
    }
}

/// C's usual arithmetic conversions for two integer types (LP64).
fn common(a: &CType, b: &CType) -> CType {
    let rank = |t: &CType| match t {
        CType::Long | CType::ULong => 2,
        CType::LongLong | CType::ULongLong => 3,
        _ => 1,
    };
    let (ra, rb) = (rank(a), rank(b));
    let (bits_a, signed_a) = bits_and_sign(a);
    let (bits_b, signed_b) = bits_and_sign(b);
    let promoted = |t: &CType| if rank(t) == 1 && bits_and_sign(t).0 < 32 { CType::Int } else { t.clone() };
    if ra == 1 && rb == 1 && signed_a && signed_b && bits_a <= 32 && bits_b <= 32 {
        return CType::Int;
    }
    let (wide, other) = if ra >= rb { (promoted(a), promoted(b)) } else { (promoted(b), promoted(a)) };
    let (_, wide_signed) = bits_and_sign(&wide);
    let (_, other_signed) = bits_and_sign(&other);
    if !wide_signed || other_signed || bits_a.max(bits_b) > 32 && wide_signed && rank(&wide) > rank(&other) {
        wide
    } else {
        match wide {
            CType::Int => CType::UInt,
            CType::Long => CType::ULong,
            CType::LongLong => CType::ULongLong,
            other => other,
        }
    }
}

fn arithmetic(op: &str, left: Value, right: Value) -> Option<Value> {
    match (left, right) {
        (Value::Int(a, ta), Value::Int(b, tb)) => {
            let ty = if matches!(op, "<<" | ">>") {
                if bits_and_sign(&ta).0 < 32 { CType::Int } else { ta.clone() }
            } else {
                common(&ta, &tb)
            };
            let value = match op {
                "+" => a + b,
                "-" => a - b,
                "*" => a.checked_mul(b)?,
                "/" if b != 0 => a / b,
                "%" if b != 0 => a % b,
                "<<" if (0..64).contains(&b) => a << b,
                ">>" if (0..64).contains(&b) => a >> b,
                "&" => a & b,
                "|" => a | b,
                "^" => a ^ b,
                "==" => return Some(Value::Int((a == b) as i128, CType::Int)),
                "!=" => return Some(Value::Int((a != b) as i128, CType::Int)),
                "<" => return Some(Value::Int((a < b) as i128, CType::Int)),
                ">" => return Some(Value::Int((a > b) as i128, CType::Int)),
                "<=" => return Some(Value::Int((a <= b) as i128, CType::Int)),
                ">=" => return Some(Value::Int((a >= b) as i128, CType::Int)),
                "&&" => return Some(Value::Int((a != 0 && b != 0) as i128, CType::Int)),
                "||" => return Some(Value::Int((a != 0 || b != 0) as i128, CType::Int)),
                _ => return None,
            };
            Some(Value::Int(wrap(value, &ty), ty))
        }
        (left, right) => {
            let float = |v: &Value| match v {
                Value::Int(n, _) => Some((*n as f64, false)),
                Value::Float(f, single) => Some((*f, *single)),
                Value::Text(_) => None,
            };
            let ((a, sa), (b, sb)) = (float(&left)?, float(&right)?);
            let single = sa && sb;
            let value = match op {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" => a / b,
                _ => return None,
            };
            Some(Value::Float(value, single))
        }
    }
}

// ---------------------------------------------------------------------------
// Tondo
// ---------------------------------------------------------------------------

const KEYWORDS: [&str; 41] = [
    "alias", "and", "as", "break", "const", "continue", "defer", "derive", "else", "enum", "extern", "fail", "false",
    "fn", "for", "if", "impl", "import", "in", "let", "match", "mut", "none", "not", "or", "priv", "pub", "ref",
    "return", "select", "self", "spawn", "suite", "test", "trait", "true", "type", "unsafe", "var", "with", "_",
];

/// Names of the prelude a declaration cannot take.
const PRELUDE: [&str; 33] = [
    "ok", "err", "some", "panic", "assert", "channel", "Bool", "Int", "Float", "Byte", "Char", "String", "Unit",
    "Never", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float32", "Float64", "Option",
    "Result", "Array", "Deque", "Map", "Set", "Range", "Iterator", "Pointer",
];

/// The Tondo type of a typedef from the C library every program has.
fn well_known(name: &str) -> Option<&'static str> {
    Some(match name {
        "size_t" => "ffi.CSize",
        "ssize_t" | "ptrdiff_t" | "intptr_t" | "off_t" | "time_t" | "intmax_t" => "Int",
        "uintptr_t" | "uintmax_t" => "UInt64",
        "int8_t" => "Int8",
        "int16_t" => "Int16",
        "int32_t" => "Int32",
        "int64_t" => "Int",
        "uint8_t" => "UInt8",
        "uint16_t" => "UInt16",
        "uint32_t" => "UInt32",
        "uint64_t" => "UInt64",
        "bool" => "Bool",
        _ => return None,
    })
}

/// A Tondo name for a C name, which keeps it unless it is a keyword or a
/// name of the prelude.
fn tondo_name(name: &str) -> String {
    if KEYWORDS.contains(&name) || PRELUDE.contains(&name) { format!("{name}_") } else { name.to_string() }
}

/// How a C type is written in Tondo, or why it cannot be.
type Mapped = Result<String, String>;

/// Where a type appears, which decides how strings and pointers map.
#[derive(Copy, Clone, PartialEq)]
enum Position {
    Parameter,
    Result,
    Field,
}

struct Emitter<'c> {
    c: &'c C,
    /// The Tondo name of each record, and of each enum with a name.
    record_names: HashMap<usize, String>,
    enum_names: HashMap<usize, String>,
    /// Records of other files the module needs: by value (`true`) or only
    /// behind pointers.
    needed: Vec<(usize, bool)>,
    /// What the module leaves out, and why.
    skipped: Vec<String>,
    used_types: HashSet<String>,
    used_values: HashSet<String>,
}

impl<'c> Emitter<'c> {
    fn new(c: &'c C) -> Emitter<'c> {
        let mut record_names = HashMap::new();
        let mut enum_names = HashMap::new();
        // A typedef that names a struct or an enum gives it its name.
        for typedef in &c.typedefs {
            match &typedef.ty {
                CType::Record(id) => {
                    record_names.entry(*id).or_insert_with(|| typedef.name.clone());
                }
                CType::Enum(id) => {
                    enum_names.entry(*id).or_insert_with(|| typedef.name.clone());
                }
                _ => {}
            }
        }
        for (id, record) in c.records.iter().enumerate() {
            if let Some(tag) = &record.tag {
                record_names.entry(id).or_insert_with(|| tag.clone());
            }
        }
        for (id, e) in c.enums.iter().enumerate() {
            if let Some(tag) = &e.tag {
                enum_names.entry(id).or_insert_with(|| tag.clone());
            }
        }
        Emitter {
            c,
            record_names,
            enum_names,
            needed: Vec::new(),
            skipped: Vec::new(),
            used_types: HashSet::new(),
            used_values: HashSet::new(),
        }
    }

    fn target(&self, file: usize) -> bool {
        self.c.targets.contains(&file)
    }

    /// The Tondo type of `ty` at `position`.
    fn ty(&mut self, ty: &CType, position: Position) -> Mapped {
        Ok(match ty {
            CType::Void => match position {
                Position::Result => "Unit".into(),
                _ => return Err("it uses `void` as a value".into()),
            },
            CType::Bool => "ffi.CBool".into(),
            CType::Char => "ffi.CChar".into(),
            CType::SChar => "Int8".into(),
            CType::UChar => "UInt8".into(),
            CType::Short => "ffi.CShort".into(),
            CType::UShort => "ffi.CUShort".into(),
            CType::Int => "ffi.CInt".into(),
            CType::UInt => "ffi.CUInt".into(),
            CType::Long => "ffi.CLong".into(),
            CType::ULong => "ffi.CULong".into(),
            CType::LongLong => "ffi.CLongLong".into(),
            CType::ULongLong => "ffi.CULongLong".into(),
            CType::Float => "ffi.CFloat".into(),
            CType::Double => "ffi.CDouble".into(),
            CType::Unsupported("va_list") => return self.va_list(position),
            CType::Unsupported(what) => return Err(format!("it uses `{what}`, which Tondo cannot pass")),
            CType::Hole => return Err("an incomplete declarator".into()),
            CType::Named(name) => return self.named(name, position),
            CType::Record(id) => {
                let record = &self.c.records[*id];
                if record.fields.as_ref().is_some_and(|f| f.iter().any(|f| f.bits.is_some())) {
                    return Err("it passes a struct with bit fields by value".into());
                }
                if record.fields.is_none() {
                    return Err("it passes an incomplete struct by value".into());
                }
                self.need(*id, true);
                self.record_name(*id)
            }
            CType::Enum(id) => match self.enum_names.get(id) {
                Some(name) if self.target(self.c.enums[*id].file) => tondo_name(name),
                _ => "ffi.CInt".into(),
            },
            CType::Array(..) => return Err("it uses an array as a value; C passes a pointer to it".into()),
            CType::Function(_) => return Err("it uses a function type as a value".into()),
            CType::Pointer(inner, pointee_const) => {
                let inner = self.resolve(inner);
                match &inner {
                    // Text C reads; text C gives back.
                    CType::Char if *pointee_const || position == Position::Result => "ffi.CString?".into(),
                    CType::Void => "Pointer[ffi.Void]?".into(),
                    // What a `va_list` parameter is on x86-64.
                    CType::Record(id) if self.c.records[*id].tag.as_deref() == Some("__va_list_tag") => {
                        self.va_list(position)?
                    }
                    CType::Function(function) => format!("({})?", self.function_type(function)?),
                    CType::Record(id) => {
                        self.need(*id, false);
                        format!("Pointer[{}]?", self.record_name(*id))
                    }
                    // A pointer to an array points to its first element.
                    CType::Array(element, _) => format!("Pointer[{}]?", self.ty(element, Position::Field)?),
                    _ => format!("Pointer[{}]?", self.ty(&inner, Position::Field)?),
                }
            }
        })
    }

    /// A typedef name: its own name when the module binds it, otherwise the
    /// Tondo type of what it names.
    fn named(&mut self, name: &str, position: Position) -> Mapped {
        if matches!(name, "va_list" | "__gnuc_va_list" | "__builtin_va_list") {
            return self.va_list(position);
        }
        if let Some(mapped) = well_known(name) {
            return Ok(mapped.into());
        }
        let Some(&index) = self.c.typedef_names.get(name) else {
            return Err(format!("it uses an unknown type `{name}`"));
        };
        let typedef = &self.c.typedefs[index];
        if self.target(typedef.file) {
            // Validate what it names, so an alias that cannot exist is not used.
            match &typedef.ty {
                CType::Record(_) | CType::Enum(_) => {}
                CType::Function(_) => return Err(format!("it uses the function type `{name}` as a value")),
                ty => {
                    self.ty(&ty.clone(), Position::Field)?;
                }
            }
            return Ok(tondo_name(name));
        }
        let ty = typedef.ty.clone();
        self.ty(&ty, position)
    }

    /// A `va_list` C passes along: as a parameter it is the address of one
    /// on every target.
    fn va_list(&self, position: Position) -> Mapped {
        match position {
            Position::Parameter => Ok("Pointer[ffi.VaList]".into()),
            _ => Err("it holds a `va_list`, which only C makes and passes along".into()),
        }
    }

    /// `ty` through typedefs of other files, to see what a pointer points to.
    fn resolve(&self, ty: &CType) -> CType {
        match ty {
            CType::Named(name) => match self.c.typedef_names.get(name) {
                Some(&index) if !self.target(self.c.typedefs[index].file) => self.resolve(&self.c.typedefs[index].ty),
                Some(&index) if matches!(self.c.typedefs[index].ty, CType::Function(_)) => {
                    self.c.typedefs[index].ty.clone()
                }
                _ => ty.clone(),
            },
            other => other.clone(),
        }
    }

    /// `ty`, or the array a chain of typedefs names.
    fn array_through_typedefs(&self, ty: &CType) -> CType {
        let mut current = ty;
        while let CType::Named(name) = current {
            match self.c.typedef_names.get(name) {
                Some(&index) => current = &self.c.typedefs[index].ty,
                None => break,
            }
        }
        if matches!(current, CType::Array(..)) { current.clone() } else { ty.clone() }
    }

    fn record_name(&self, id: usize) -> String {
        match self.record_names.get(&id) {
            Some(name) => tondo_name(name),
            None => format!("Anonymous{id}"),
        }
    }

    fn need(&mut self, id: usize, by_value: bool) {
        if self.target(self.c.records[id].file) && self.record_names.contains_key(&id) {
            return;
        }
        let own = self.c.complete_records && !self.record_name(id).starts_with('_');
        let by_value = by_value || own && self.c.records[id].fields.is_some();
        match self.needed.iter_mut().find(|(n, _)| *n == id) {
            Some((_, value)) => *value |= by_value,
            None => self.needed.push((id, by_value)),
        }
    }

    fn function_type(&mut self, function: &CFunction) -> Mapped {
        let params: Vec<String> =
            function.params.iter().map(|(_, ty)| self.ty(ty, Position::Parameter)).collect::<Result<_, _>>()?;
        let mut params = params;
        if function.variadic {
            params.push("...".into());
        }
        let result = self.ty(&function.result, Position::Result)?;
        Ok(if result == "Unit" {
            format!("extern fn({})", params.join(", "))
        } else {
            format!("extern fn({}): {result}", params.join(", "))
        })
    }

    fn module(mut self, command: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("// Generated by `{command}`; run it again to update this module.\n\n"));
        out.push_str("import std.ffi\n\n");
        let mut body = Vec::new();
        for &(kind, index) in &self.c.order {
            let declaration = match kind {
                Kind::Function => self.function(index),
                Kind::Typedef => self.typedef(index),
                Kind::Record => self.record(index, false),
                Kind::Enum => self.enumeration(index),
            };
            body.extend(declaration);
        }
        body.extend(self.macros());
        // Records of other headers the declarations reach.
        let mut done = HashSet::new();
        while let Some((id, by_value)) = self.needed.pop() {
            if !done.insert(id) {
                continue;
            }
            let declaration = if by_value { self.record(id, true) } else { None };
            match declaration {
                Some(declaration) => body.push(declaration),
                None if self.used_types.insert(self.record_name(id)) => {
                    body.push(format!("pub extern type {}", self.record_name(id)))
                }
                None => {}
            }
        }
        out.push_str(&body.join("\n\n"));
        out.push('\n');
        if !self.skipped.is_empty() {
            out.push_str("\n// Not bound:\n");
            for skipped in &self.skipped {
                out.push_str(&format!("// - {skipped}\n"));
            }
        }
        out
    }

    fn function(&mut self, index: usize) -> Option<String> {
        let function = &self.c.functions[index];
        if !self.target(function.file) || function.inline || function.name.starts_with("__") {
            return None;
        }
        let name = tondo_name(&function.name);
        if !self.used_values.insert(name.clone()) {
            return None;
        }
        let mut params = Vec::new();
        let mut names = HashSet::new();
        for (index, (param, ty)) in function.ty.params.iter().enumerate() {
            let mut param = param.as_deref().map(tondo_name).unwrap_or_else(|| format!("arg{}", index + 1));
            if !names.insert(param.clone()) {
                param = format!("{param}{}", index + 1);
            }
            match self.ty(ty, Position::Parameter) {
                Ok(ty) => params.push(format!("{param}: {ty}")),
                Err(reason) => {
                    self.skipped.push(format!("{}: {reason}", function.name));
                    return None;
                }
            }
        }
        if function.ty.variadic {
            params.push("...".into());
        }
        let result = match self.ty(&function.ty.result, Position::Result) {
            Ok(result) => result,
            Err(reason) => {
                self.skipped.push(format!("{}: {reason}", function.name));
                return None;
            }
        };
        let symbol = match &function.symbol {
            // On macOS an `__asm__` label carries the underscore Mach-O adds.
            Some(label) if cfg!(target_os = "macos") => Some(label.strip_prefix('_').unwrap_or(label).to_string()),
            Some(label) => Some(label.clone()),
            None if name != function.name => Some(function.name.clone()),
            None => None,
        };
        let symbol = symbol.filter(|s| *s != name);
        let annotation = symbol.map(|s| format!("@symbol({s:?})\n")).unwrap_or_default();
        let result = if result == "Unit" { String::new() } else { format!(": {result}") };
        Some(format!("{annotation}pub extern fn {name}({}){result}", params.join(", ")))
    }

    fn typedef(&mut self, index: usize) -> Option<String> {
        let typedef = &self.c.typedefs[index];
        if !self.target(typedef.file) || typedef.name.starts_with("__") || well_known(&typedef.name).is_some() {
            return None;
        }
        let name = tondo_name(&typedef.name);
        match &typedef.ty {
            // The struct or enum takes the typedef's name.
            CType::Record(id) if self.record_names.get(id) == Some(&typedef.name) => {
                return self.record(*id, false);
            }
            CType::Enum(id) if self.enum_names.get(id) == Some(&typedef.name) => return self.enumeration(*id),
            _ => {}
        }
        if !self.used_types.insert(name.clone()) {
            return None;
        }
        let ty = typedef.ty.clone();
        let mapped = match &ty {
            CType::Function(function) => self.function_type(function),
            // A struct named by its tag elsewhere: `typedef struct x y`.
            CType::Record(id) if self.c.records[*id].fields.is_none() => {
                return Some(format!("pub extern type {name}"));
            }
            _ => self.ty(&ty, Position::Field),
        };
        match mapped {
            Ok(mapped) => Some(format!("pub alias {name} = {mapped}")),
            Err(reason) => {
                self.skipped.push(format!("{}: {reason}", typedef.name));
                None
            }
        }
    }

    /// An `extern type` for a struct, opaque when it has no body; a union
    /// or a struct with bit fields becomes opaque bytes of its size.
    fn record(&mut self, id: usize, dependency: bool) -> Option<String> {
        let record = &self.c.records[id];
        if !dependency && !self.target(record.file) {
            return None;
        }
        let name = self.record_name(id);
        if record.tag.is_none() && !self.record_names.contains_key(&id) && !dependency {
            return None;
        }
        if !self.used_types.insert(name.clone()) {
            return None;
        }
        let Some(fields) = &record.fields else { return Some(format!("pub extern type {name}")) };
        let bytes = |c: &C, what: &str| {
            let (size, align) = layout(c, &CType::Record(id));
            let align = if align > 1 { format!("@align({align})\n") } else { String::new() };
            Some(format!(
                "// {what}: its {size} bytes, which C reads and writes\n{align}pub extern type {name} = {{\n    @count({size})\n    bytes: UInt8\n}}"
            ))
        };
        if fields.iter().any(|f| f.bits.is_some()) {
            return bytes(self.c, "a struct with bit fields");
        }
        let union = record.union;
        let mut lines = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            let field_name = field.name.clone().unwrap_or_else(|| format!("anonymous{index}"));
            let (ty, count) = match &self.array_through_typedefs(&field.ty) {
                CType::Array(element, Some(count)) => {
                    // A nested array is flat in memory.
                    let mut element = (**element).clone();
                    let mut count = *count;
                    while let CType::Array(inner, Some(n)) = element {
                        count *= n;
                        element = *inner;
                    }
                    (element, Some(count))
                }
                CType::Array(_, None) => continue,
                ty => (ty.clone(), None),
            };
            let mapped = match &ty {
                CType::Record(inner)
                    if self.c.records[*inner].tag.is_none() && !self.record_names.contains_key(inner) =>
                {
                    let inner_name = format!("{name}_{field_name}");
                    self.record_names.insert(*inner, inner_name.clone());
                    self.needed.push((*inner, true));
                    Ok(inner_name)
                }
                _ => self.ty(&ty, Position::Field),
            };
            match mapped {
                Ok(mapped) => {
                    let count = count.map(|c| format!("@count({c})\n    ")).unwrap_or_default();
                    lines.push(format!("    {count}{field_name}: {mapped}"));
                }
                Err(reason) if union => {
                    self.skipped.push(format!("{name}: field `{field_name}`: {reason}; the union is its bytes"));
                    return bytes(self.c, "a union");
                }
                Err(reason) => {
                    self.skipped.push(format!("{name}: field `{field_name}`: {reason}; the struct is opaque"));
                    return Some(format!("pub extern type {name}"));
                }
            }
        }
        let annotation = if union { "@union\n" } else { "" };
        Some(format!("{annotation}pub extern type {name} = {{\n{}\n}}", lines.join("\n")))
    }

    fn enumeration(&mut self, id: usize) -> Option<String> {
        let e = &self.c.enums[id];
        if !self.target(e.file) {
            return None;
        }
        let mut out = Vec::new();
        let ty = match self.enum_names.get(&id) {
            Some(name) => {
                let name = tondo_name(name);
                if self.used_types.insert(name.clone()) {
                    out.push(format!("pub alias {name} = ffi.CInt"));
                }
                name
            }
            None => "ffi.CInt".into(),
        };
        for (item, value) in &e.items {
            let Some(value) = value else {
                self.skipped.push(format!("{item}: its value is not a constant this tool evaluates"));
                continue;
            };
            let name = tondo_name(item);
            if self.used_values.insert(name.clone()) {
                out.push(format!("pub const {name}: {ty} = {value}"));
            }
        }
        (!out.is_empty()).then(|| out.join("\n\n"))
    }

    /// The object-like macros of the bound headers that are constants.
    fn macros(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for m in &self.c.macros {
            if !self.target(m.file) || m.body.is_empty() || m.name.starts_with('_') {
                continue;
            }
            if let Some(params) = &m.params {
                if let Some(function) = self.macro_function(&m.name, params, &m.body) {
                    out.push(function);
                }
                continue;
            }
            let name = tondo_name(&m.name);
            if self.used_values.contains(&name) {
                continue;
            }
            let value = Evaluator::new(self.c).value(&m.body);
            let line = match value {
                Some(Value::Int(n, ty)) => {
                    let ty = self.ty(&ty, Position::Field).unwrap_or_else(|_| "ffi.CInt".into());
                    format!("pub const {name}: {ty} = {n}")
                }
                Some(Value::Float(f, single)) if f.is_finite() => {
                    let ty = if single { "ffi.CFloat" } else { "ffi.CDouble" };
                    let mut text = format!("{f}");
                    if !text.contains('.') {
                        text.push_str(".0");
                    }
                    format!("pub const {name}: {ty} = {text}")
                }
                Some(Value::Float(..)) => continue,
                Some(Value::Text(text)) => format!("pub const {name}: String = {}", quote(&text)),
                None => {
                    if let Some(pointer) = self.pointer_macro(&name, &m.body) {
                        self.used_values.insert(name);
                        out.push(pointer);
                        continue;
                    }
                    // A value that is not a number or text.
                    if m.body.iter().any(|t| matches!(t.tok, Tok::Number(_))) {
                        let text = spell(&m.body);
                        self.skipped.push(format!("{}: `{text}` is not a number or text a constant can hold", m.name));
                    }
                    continue;
                }
            };
            self.used_values.insert(name);
            out.push(line);
        }
        out
    }
}

impl Emitter<'_> {
    /// A macro that is a pointer made from a number: `none` for `NULL`, and
    /// a function that makes it otherwise, since a constant cannot hold a
    /// pointer.
    fn pointer_macro(&mut self, name: &str, body: &[Token]) -> Option<String> {
        let (ty, value) = cast(self.c, body)?;
        let value = Evaluator::new(self.c).integer(&value)?;
        let resolved = match self.resolve(&ty) {
            CType::Named(n) => self.c.typedef_names.get(&n).map(|&i| self.c.typedefs[i].ty.clone())?,
            other => other,
        };
        let CType::Pointer(pointee, _) = resolved else { return None };
        let mapped = self.ty(&ty, Position::Field).ok()?;
        if value == 0 {
            return Some(format!("pub const {name}: {mapped} = none"));
        }
        let address = value as u64;
        let made = match self.resolve(&pointee) {
            CType::Function(function) => format!("toFunction[{}]", self.function_type(&function).ok()?),
            CType::Void => "toPointer[ffi.Void]".into(),
            other => format!("toPointer[{}]", self.ty(&other, Position::Field).ok()?),
        };
        Some(format!("pub fn {name}(): {mapped} {{\n    unsafe {{ {address}u64.{made}() }}\n}}"))
    }

    /// A function-like macro that calls a bound function, as a function of
    /// Tondo whose parameters take the types of the arguments they become.
    fn macro_function(&mut self, name: &str, params: &[String], body: &[Token]) -> Option<String> {
        let tondo = tondo_name(name);
        if self.used_values.contains(&tondo) {
            return None;
        }
        // Other macros with parameters, such as helpers of the header, are
        // not part of its interface.
        let (callee, args) = call(body)?;
        let Some(function) =
            self.c.functions.iter().find(|f| f.name == callee && self.used_values.contains(&tondo_name(&f.name)))
        else {
            self.skipped.push(format!("{name}: a macro that calls `{callee}`, which is not bound"));
            return None;
        };
        let (callee_params, result) = (function.ty.params.clone(), function.ty.result.clone());
        if args.len() != callee_params.len() {
            self.skipped.push(format!("{name}: a macro whose call does not match `{callee}`"));
            return None;
        }
        let mut types: Vec<Option<String>> = vec![None; params.len()];
        let mut values = Vec::new();
        for (arg, (_, param_ty)) in args.iter().zip(&callee_params) {
            // A parameter, perhaps in parentheses or cast.
            let bare = unwrap(arg);
            let bare = match cast(self.c, bare) {
                Some((_, rest)) if rest.len() == 1 => rest,
                _ => bare.to_vec(),
            };
            let parameter = match bare.as_slice() {
                [Token { tok: Tok::Ident(p), .. }] => params.iter().position(|q| q == p),
                _ => None,
            };
            if let Some(index) = parameter {
                // Text passes as a `String`, which the call of C copies.
                let ty = match self.ty(param_ty, Position::Parameter).ok()? {
                    ty if ty == "ffi.CString?" || ty == "ffi.CString" => "String".to_string(),
                    ty => ty,
                };
                types[index] = Some(ty);
                values.push(tondo_name(&params[index]));
                continue;
            }
            // A constant the module declares, by its name.
            if let [Token { tok: Tok::Ident(constant), .. }] = unwrap(arg)
                && self.used_values.contains(&tondo_name(constant))
            {
                values.push(tondo_name(constant));
                continue;
            }
            match Evaluator::new(self.c).value(arg) {
                Some(Value::Int(n, _)) => values.push(n.to_string()),
                Some(Value::Float(f, _)) => values.push(format!("{f:?}")),
                Some(Value::Text(text)) => values.push(quote(&text)),
                None => {
                    self.skipped.push(format!("{name}: an argument `{}` this tool does not evaluate", spell(arg)));
                    return None;
                }
            }
        }
        let Some(types) = types.into_iter().collect::<Option<Vec<String>>>() else {
            self.skipped.push(format!("{name}: a macro parameter that is not an argument of `{callee}`"));
            return None;
        };
        let result = self.ty(&result, Position::Result).ok()?;
        let result = if result == "Unit" { String::new() } else { format!(": {result}") };
        let params: Vec<String> = params.iter().zip(&types).map(|(p, t)| format!("{}: {t}", tondo_name(p))).collect();
        self.used_values.insert(tondo.clone());
        Some(format!(
            "pub unsafe fn {tondo}({}){result} {{\n    {}({})\n}}",
            params.join(", "),
            tondo_name(&callee),
            values.join(", ")
        ))
    }
}

/// C tokens as text, to show them.
fn spell(tokens: &[Token]) -> String {
    let mut out = String::new();
    for token in tokens {
        let text = match &token.tok {
            Tok::Ident(word) | Tok::Number(word) => word.clone(),
            Tok::Char(c) => format!("'{}'", char::from_u32(*c as u32).unwrap_or('?')),
            Tok::Str(text) => format!("{text:?}"),
            Tok::Punct(p) => p.to_string(),
        };
        let word = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if word(&text) && out.chars().last().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.push(' ');
        }
        out.push_str(&text);
    }
    out
}

/// A string literal of Tondo for `text`.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The size and alignment of a C type (LP64).
fn layout(c: &C, ty: &CType) -> (u64, u64) {
    match ty {
        CType::Bool | CType::Char | CType::SChar | CType::UChar => (1, 1),
        CType::Short | CType::UShort => (2, 2),
        CType::Int | CType::UInt | CType::Float | CType::Enum(_) => (4, 4),
        CType::Pointer(..) | CType::Long | CType::ULong | CType::LongLong | CType::ULongLong | CType::Double => (8, 8),
        CType::Unsupported("long double" | "__int128") => (16, 16),
        CType::Named(name) => match c.typedef_names.get(name) {
            Some(&index) => layout(c, &c.typedefs[index].ty),
            None => (8, 8),
        },
        CType::Array(element, count) => {
            let (size, align) = layout(c, element);
            (size * count.unwrap_or(0), align)
        }
        CType::Record(id) => {
            // In bits, as GCC and Clang place bit fields: in the next bits
            // that do not cross a unit of the field's type.
            let record = &c.records[*id];
            let (mut bit, mut size, mut align) = (0u64, 0u64, 1u64);
            for field in record.fields.iter().flatten() {
                let (field_size, field_align) = layout(c, &field.ty);
                let start = if record.union { 0 } else { bit };
                let end = match field.bits {
                    Some(0) => start.next_multiple_of(field_align * 8),
                    Some(width) => {
                        let unit = field_size * 8;
                        let at = if start % unit + width > unit { start.next_multiple_of(unit) } else { start };
                        if field.name.is_some() {
                            align = align.max(field_align);
                        }
                        at + width
                    }
                    None => {
                        align = align.max(field_align);
                        start.next_multiple_of(field_align * 8) + field_size * 8
                    }
                };
                size = size.max(end);
                bit = end;
            }
            (size.div_ceil(8).next_multiple_of(align), align)
        }
        _ => (8, 8),
    }
}

/// The path of the module `tondo bind` writes for a header in a project.
pub fn module_path(project: &Path, header: &Path) -> (String, PathBuf) {
    let stem = header.file_stem().and_then(|s| s.to_str()).unwrap_or("native");
    let mut name: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if name.starts_with(|c: char| c.is_ascii_digit()) || KEYWORDS.contains(&name.as_str()) {
        name.insert(0, 'c');
    }
    let root = if project.join("src").is_dir() { project.join("src") } else { project.to_path_buf() };
    let path = root.join(&name).join(format!("{name}.to"));
    (name, path)
}

/// Adds a library to `[native] libraries` of the project's `tondo.toml`,
/// creating the table or the file when needed.
pub fn add_library(project: &Path, library: &str) -> Result<bool, String> {
    let path = project.join("tondo.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let tables = crate::toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let existing = tables
        .iter()
        .find(|t| t.name == "native")
        .and_then(|t| t.entries.iter().find(|(k, _, _)| k == "libraries").map(|(_, v, line)| (v.clone(), *line)));
    let quoted = crate::toml::quote(library);
    let updated = match existing {
        Some((crate::toml::Value::Array(items), line)) => {
            if items.iter().any(|i| i.as_str() == Some(library)) {
                return Ok(false);
            }
            let mut entries: Vec<String> = items.iter().filter_map(|i| i.as_str()).map(crate::toml::quote).collect();
            entries.push(quoted);
            let lines: Vec<&str> = text.lines().collect();
            let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
            out[line - 1] = format!("libraries = [{}]", entries.join(", "));
            out.join("\n") + "\n"
        }
        Some(_) => return Err(format!("{}: `[native] libraries` must be an array", path.display())),
        None if tables.iter().any(|t| t.name == "native") => {
            let mut out = Vec::new();
            for line in text.lines() {
                out.push(line.to_string());
                if line.trim() == "[native]" {
                    out.push(format!("libraries = [{quoted}]"));
                }
            }
            out.join("\n") + "\n"
        }
        None => {
            let mut out = text.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[native]\nlibraries = [{quoted}]\n"));
            out
        }
    };
    std::fs::write(&path, updated).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}
