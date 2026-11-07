//! The formats others read, as recorded in `tests/snapshots/formats`: the
//! report of a panic as text and as JSON, a dump, what `tondo dump read`
//! gives of it, and the JSON report of `tondo test`. Where the values vary,
//! as times and addresses, what is held is the shape: each field's path and
//! the kind of its value (docs/refactor.md, phase 0.4). `TONDO_UPDATE_FORMATS=1`
//! records them again, on purpose.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const FALLS: &str = "import std.console

context Request = {
    @log id: String = \"\"
}

type Order = {
    id: Int
    items: Array[String]
}

fn total(order: Order): Int {
    let prices = [3, 4]
    prices[order.items.length()]
}

fn main() {
    with Request { id: \"r-7\" }
    let order = Order { id: 42, items: [\"a\", \"b\", \"c\"] }
    let job = spawn total(order)
    console.println(\"{job.wait()}\")
}
";

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tondo(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
    command.args(args).current_dir(dir).env_remove("TONDO_DUMP").env_remove("TONDO_PANIC_FORMAT");
    command.envs(env.iter().copied());
    let output = command.output().unwrap();
    (String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

/// Each field's path and the kind of its value, a line each; a key that is
/// an address is `<address>`, and below a path of `vary` nothing is held.
fn shape(value: &Value, vary: &[&str]) -> String {
    fn walk(value: &Value, path: String, vary: &[&str], out: &mut BTreeSet<String>) {
        if vary.contains(&path.as_str()) {
            out.insert(format!("{path} (varies)"));
            return;
        }
        match value {
            Value::Object(fields) => {
                for (key, inner) in fields {
                    let address = key.len() >= 8 && key.chars().all(|c| c.is_ascii_hexdigit());
                    let key = if address { "<address>" } else { key.as_str() };
                    walk(inner, format!("{path}.{key}"), vary, out);
                }
            }
            Value::Array(items) => {
                out.insert(format!("{path}[]"));
                for item in items {
                    walk(item, format!("{path}[]"), vary, out);
                }
            }
            Value::Null => _ = out.insert(format!("{path}: null")),
            Value::Bool(_) => _ = out.insert(format!("{path}: bool")),
            Value::Number(_) => _ = out.insert(format!("{path}: number")),
            Value::String(_) => _ = out.insert(format!("{path}: string")),
        }
    }
    let mut out = BTreeSet::new();
    walk(value, String::new(), vary, &mut out);
    out.into_iter().map(|line| line + "\n").collect()
}

/// `text` as recorded under `name`, or recorded with `TONDO_UPDATE_FORMATS`.
fn recorded(name: &str, text: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/formats").join(name);
    if std::env::var_os("TONDO_UPDATE_FORMATS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(before, text, "{name} changed; if on purpose, run with TONDO_UPDATE_FORMATS=1");
}

#[test]
fn a_panic_and_its_dump_keep_their_formats() {
    let dir = scratch("formats-panic");
    std::fs::write(dir.join("falls.to"), FALLS).unwrap();
    let dumps = dir.join("dumps");
    std::fs::create_dir_all(&dumps).unwrap();
    // The report as text, with where the dump went.
    let (_, said) = tondo(&dir, &["run", "falls.to"], &[("TONDO_DUMP", dumps.to_str().unwrap())]);
    let dump = std::fs::read_dir(&dumps).unwrap().next().unwrap().unwrap().path();
    let said = said.replace(dump.to_str().unwrap(), "<dump>");
    recorded("panic.txt", &said);
    // As JSON.
    let (_, said) = tondo(&dir, &["run", "falls.to"], &[("TONDO_DUMP", "off"), ("TONDO_PANIC_FORMAT", "json")]);
    let panic: Value = serde_json::from_str(said.lines().next().unwrap()).unwrap();
    recorded("panic.json", &(serde_json::to_string_pretty(&panic).unwrap() + "\n"));
    // The dump, whose samples of memory are taken by chance.
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&dump).unwrap()).unwrap();
    recorded("dump.shape", &shape(&written, &[".heap.samples", ".events"]));
    // What `tondo dump read` gives of it, as the tools read it.
    for view in ["why", "tasks", "locals"] {
        let (read, _) = tondo(&dir, &["dump", "read", dump.to_str().unwrap(), view, "--json"], &[]);
        let read: Value = serde_json::from_str(&read).unwrap_or_else(|_| panic!("{view}: {read}"));
        recorded(&format!("dump-{view}.shape"), &shape(&read, &[]));
    }
}

#[test]
fn the_report_of_a_test_run_keeps_its_format() {
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/testing/sample");
    let (report, said) = tondo(&sample, &["test", "--test-format", "json", "--timeout", "1"], &[("TONDO_DUMP", "off")]);
    let report: Value = serde_json::from_str(&report).unwrap_or_else(|_| panic!("{report}{said}"));
    recorded("test-report.shape", &shape(&report, &[]));
}
