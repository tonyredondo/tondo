//! Typed plugins (standard library 14.48): one implementation of a
//! contract offered by a library and by a program, each method called both
//! ways with the same results, from several tasks at once; a plugin that
//! panics, one that ends in the middle of a call, one that ends with the
//! last copy of its value, and one built against another contract.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/api")).unwrap();
    dir
}

const CONTRACT: &str = r#"import std.plugin

pub type Note = { text: String, loud: Bool }

pub enum Refusal {
    Busy
    Rude(String)
}

/// What greets.
@plugin.Contract
pub trait Greeter {
    fn greet(self, name: String): String ! plugin.Error
    fn note(self, note: Note, times: Int): Array[String] ! (Refusal | plugin.Error)
    fn quit(self): !plugin.Error
}
"#;

const PROGRAM: &str = r#"import greet.api
import std.console
import std.env
import std.ffi
import std.fs
import std.plugin
import std.process

type Polite = {}

impl api.Greeter for Polite {
    fn greet(self, name: String): String ! plugin.Error {
        "good morning, {name}"
    }

    fn note(self, note: api.Note, times: Int): Array[String] ! (api.Refusal | plugin.Error) {
        if times > 3 {
            fail api.Refusal.Rude("too many")
        }
        if times < 0 {
            panic("a negative count")
        }
        [if note.loud { note.text.toUpper() } else { note.text }].repeat(times)
    }

    fn quit(self): !plugin.Error {
        process.exit(0)
    }
}

@plugin.Export
fn greeter(): impl api.Greeter {
    Polite {}
}

/// Every method, and many calls at once, of a plugin however loaded.
fn calls(plugged: api.GreeterPlugin) {
    console.println("{plugged.greet("Ada")}")
    console.println("{plugged.note(api.Note { text: "hi", loud: true }, 2)}")
    console.println("{plugged.note(api.Note { text: "hi", loud: false }, 5)}")
    console.println("{plugged.note(api.Note { text: "hi", loud: false }, -1)}")
    var group = Group[String ! plugin.Error]()
    for index in 0..16 {
        group.add(spawn plugged.greet("caller {index}"))
    }
    match group.wait() {
        ok(answers) => console.println("{answers.length()} answers, the last {answers[15]}")
        err(error) => console.println("{error}")
    }
}

fn main() {
    let args = env.args()
    match args.get(0) ?? "" {
        "--serve" => {
            _ = api.serveGreeter(Polite {})
            // Only once the other side has closed.
            _ = fs.writeText(args[1], "ended")
        }
        "--library" => match api.GreeterPlugin.load(args[1]) {
            ok(loaded) => calls(loaded)
            err(error) => console.println("{error}")
        }
        "--program" => {
            let program = process.command(args[1], "--serve", args[2])
            match api.GreeterPlugin.start(program) {
                ok(started) => {
                    calls(started)
                    match started.quit() {
                        ok(_) => console.println("quit")
                        err(error) => console.println("{error}")
                    }
                }
                err(error) => console.println("{error}")
            }
            // The plugin ended when its last copy went.
            console.println("{fs.readText(args[2]) ?? "running"}")
        }
        _ => console.println("--serve, --library or --program")
    }
}
"#;

/// Builds the project in `dir` as a program and as a library.
fn built(dir: &Path, api: &str) -> (PathBuf, PathBuf) {
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"greet\"\n").unwrap();
    std::fs::write(dir.join("src/api/api.to"), api).unwrap();
    std::fs::write(dir.join("src/main.to"), PROGRAM).unwrap();
    let (_, err, status) = tondo(dir, &["build", "--output", "app"]);
    assert_eq!(status, 0, "{err}");
    let (_, err, status) = tondo(dir, &["build", "--library", "--output", "lib"]);
    assert_eq!(status, 0, "{err}");
    let library = if cfg!(target_os = "macos") { "libgreet.dylib" } else { "libgreet.so" };
    (dir.join("app"), dir.join("lib").join(library))
}

fn run(program: &Path, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_contract_crosses_to_a_library_and_to_a_program_alike() {
    let dir = scratch("plugins-both");
    let (app, library) = built(&dir, CONTRACT);
    let calls = "ok(good morning, Ada)\nok([HI, HI])\nerr(Rude(too many))\nerr(the plugin panicked: a negative count)\n\
                 16 answers, the last good morning, caller 15\n";
    assert_eq!(run(&app, &["--library", library.to_str().unwrap()]), calls);
    let marker = dir.join("ended");
    let started = run(&app, &["--program", app.to_str().unwrap(), marker.to_str().unwrap()]);
    assert_eq!(started, format!("{calls}the plugin is gone\nrunning\n"), "the plugin that quits mid-call");
    // A plugin that goes with the last copy of its value has ended by then.
    let mut quiet = PROGRAM.replace(
        "match started.quit() {\n                        ok(_) => console.println(\"quit\")\n                        err(error) => console.println(\"{error}\")\n                    }",
        "_ = 0",
    );
    quiet = quiet.replace("        _ => console.println(\"--serve, --library or --program\")", "        _ => {}");
    std::fs::write(dir.join("src/main.to"), quiet).unwrap();
    let (_, err, status) = tondo(&dir, &["build", "--output", "app"]);
    assert_eq!(status, 0, "{err}");
    let ended = run(&app, &["--program", app.to_str().unwrap(), marker.to_str().unwrap()]);
    assert!(ended.ends_with("ended\n"), "{ended}");
}

#[test]
fn a_plugin_of_another_contract_is_refused() {
    let dir = scratch("plugins-other");
    let (app, _) = built(&dir, CONTRACT);
    // The plugin built against a note with one field more.
    let other = scratch("plugins-other-plugin");
    let (plugin, library) = built(&other, &CONTRACT.replace("loud: Bool }", "loud: Bool, color: String = \"red\" }"));
    let refused = "the plugin is of another contract: this program expects `greet.api.Greeter`, and the plugin offers \
                   `greet.api.Greeter` of other methods or types: their fingerprints differ\n";
    assert_eq!(run(&app, &["--library", library.to_str().unwrap()]), refused);
    let marker = dir.join("ended");
    assert_eq!(
        run(&app, &["--program", plugin.to_str().unwrap(), marker.to_str().unwrap()]),
        format!("{refused}ended\n")
    );
}
