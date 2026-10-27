//! What a panic carries beyond its message (language 15.7): the program's
//! own code and data, which `panic(message, code:, data:)` stages here just
//! before it panics, and the forms its report takes, the text of 15.7 and
//! the JSON of `TONDO_PANIC_FORMAT=json` (toolchain 7.5).

use super::{Str, str_bytes};
use std::cell::RefCell;
use std::fmt::Write;

/// A value of a panic's data: `Int | Float | Bool | String`.
#[derive(Clone)]
pub(crate) enum Datum {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl Datum {
    /// As `Display` shows the member it holds.
    fn text(&self) -> String {
        match self {
            Datum::Int(n) => n.to_string(),
            Datum::Float(x) => format!("{x:?}"),
            Datum::Bool(b) => b.to_string(),
            Datum::Text(text) => text.clone(),
        }
    }

    /// As JSON: a number, a boolean or a string, and a float that is not
    /// finite as its text.
    fn json(&self) -> String {
        match self {
            Datum::Int(n) => n.to_string(),
            Datum::Float(x) if x.is_finite() => format!("{x:?}"),
            Datum::Float(x) => crate::diagnostics::quoted(&format!("{x:?}")),
            Datum::Bool(b) => b.to_string(),
            Datum::Text(text) => crate::diagnostics::quoted(text),
        }
    }
}

/// What the program gave the panic it is about to raise.
#[derive(Default)]
struct Staged {
    code: Option<String>,
    data: Vec<(String, Datum)>,
    /// Whether the language gave the data, from the values its message
    /// already shows, rather than the program.
    language: bool,
}

/// The data of a panic of the language, which its message already shows
/// (22.2 of the language).
pub(crate) fn language_data(data: Vec<(&str, Datum)>) {
    STAGED.with(|staged| {
        let mut staged = staged.borrow_mut();
        staged.data = data.into_iter().map(|(key, value)| (key.to_string(), value)).collect();
        staged.language = true;
    });
}

thread_local! {
    static STAGED: RefCell<Staged> = RefCell::new(Staged::default());
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_code(code: *const Str) {
    let code = text(code);
    STAGED.with(|staged| staged.borrow_mut().code = Some(code));
}

fn stage(key: *const Str, datum: Datum) {
    let key = text(key);
    STAGED.with(|staged| {
        let data = &mut staged.borrow_mut().data;
        data.retain(|(k, _)| *k != key);
        data.push((key, datum));
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_int(key: *const Str, value: i64) {
    stage(key, Datum::Int(value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_float(key: *const Str, value: f64) {
    stage(key, Datum::Float(value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_bool(key: *const Str, value: u8) {
    stage(key, Datum::Bool(value != 0));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_panic_text(key: *const Str, value: *const Str) {
    stage(key, Datum::Text(text(value)));
}

/// Whether a code is the program's own: segments of a lowercase letter
/// followed by letters and digits, separated by dots (15.7).
pub(crate) fn own_code(code: &str) -> bool {
    !code.is_empty()
        && code.split('.').all(|segment| {
            let mut chars = segment.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_alphanumeric())
        })
}

/// A panic as its report shows it.
pub(crate) struct Panic {
    /// `P0001`, or the program's own code.
    pub code: String,
    /// The stable name of a code of the language, as `bounds`.
    pub name: Option<&'static str>,
    pub message: String,
    pub data: Vec<(String, Datum)>,
    /// Whether the program gave the data, which the text then shows.
    pub given: bool,
    pub location: String,
}

impl Panic {
    /// The panic that `code` of the language raises with `message`, and
    /// what the program staged for it: its own code and its data. A code
    /// that is not one is `P0017`, with what it was as `code`.
    pub(crate) fn new(code: u32, name: &'static str, message: String, location: String) -> Panic {
        let staged = STAGED.with(|staged| std::mem::take(&mut *staged.borrow_mut()));
        let mut given = !staged.data.is_empty() && !staged.language;
        let mut data = staged.data;
        let (code, name) = match staged.code {
            Some(own) if own_code(&own) => (own, None),
            Some(other) => {
                data.retain(|(key, _)| key != "code");
                data.push(("code".into(), Datum::Text(other)));
                given = true;
                ("P0017".into(), Some("invalid-panic-code"))
            }
            None => (format!("P{code:04}"), Some(name)),
        };
        Panic { code, name, message, data, given, location }
    }

    /// The report's first lines: `panic[code]: name: message`, and the
    /// data the program gave, a line each.
    pub(crate) fn headline(&self) -> String {
        let mut text = format!("panic[{}]: ", self.code);
        if let Some(name) = self.name {
            text.push_str(name);
            if !self.message.is_empty() {
                text.push_str(": ");
            }
        }
        text.push_str(&self.message);
        if self.given {
            for (key, value) in &self.data {
                let _ = write!(text, "\n  {key}: {}", value.text());
            }
        }
        text
    }

    /// The data as a JSON object, its keys in order, or `null`.
    pub(crate) fn data_json(&self) -> String {
        let mut data: Vec<&(String, Datum)> = self.data.iter().collect();
        data.sort_by(|a, b| a.0.cmp(&b.0));
        match data.is_empty() {
            true => "null".to_string(),
            false => {
                let members: Vec<String> = data
                    .iter()
                    .map(|(key, value)| format!("{}:{}", crate::diagnostics::quoted(key), value.json()))
                    .collect();
                format!("{{{}}}", members.join(","))
            }
        }
    }

    /// The report as one line of `tondo-panic-0.1/1`, with the frames of
    /// `stack`, a JSON array.
    pub(crate) fn json(&self, stack: &str, recovered: bool, suppressed: bool) -> String {
        let data = self.data_json();
        let name = self.name.map_or("null".to_string(), crate::diagnostics::quoted);
        let mut out = format!(
            "{{\"code\":{},\"data\":{data},\"error_type\":null,\"format\":\"tondo-panic-0.1/1\",\"kind\":\"panic\",\"message\":{},\"name\":{name},\"recovered\":{recovered},\"source\":{},\"stack\":{stack}",
            crate::diagnostics::quoted(&self.code),
            crate::diagnostics::quoted(&self.message),
            source(&self.location),
        );
        if suppressed {
            out.push_str(",\"suppressed\":true");
        }
        out.push('}');
        out
    }
}

/// A `file:line:column` location as JSON, or `null`.
pub(crate) fn source(location: &str) -> String {
    let mut parts = location.rsplitn(3, ':');
    let (column, line, file) = (parts.next(), parts.next(), parts.next());
    match (file, line.and_then(|l| l.parse::<u32>().ok()), column.and_then(|c| c.parse::<u32>().ok())) {
        (Some(file), Some(line), Some(column)) => {
            format!("{{\"column\":{column},\"file\":{},\"line\":{line}}}", crate::diagnostics::quoted(file))
        }
        _ => "null".into(),
    }
}

/// Whether the program reports its failures as JSON, for tools.
pub(crate) fn json_reports() -> bool {
    static JSON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *JSON.get_or_init(|| std::env::var_os("TONDO_PANIC_FORMAT").is_some_and(|v| v == "json"))
}
