//! `tondo lsp` (toolchain 7.4) as an editor uses it, over standard input
//! and output.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Editor {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    /// The diagnostics last published for each file.
    diagnostics: Vec<(String, Value)>,
    /// The tests of projects sent, in order (`tondo/tests`).
    tests: Vec<Value>,
}

impl Editor {
    fn start() -> Editor {
        Editor::start_with(Value::Null)
    }

    /// An editor with a folder open, whose projects the server analyzes.
    fn start_with_folders(folder: &Path) -> Editor {
        Editor::launch(Value::Null, json!([{ "uri": format!("file://{}", folder.display()), "name": "folder" }]))
    }

    /// An editor that gives the server `initializationOptions`.
    fn start_with(options: Value) -> Editor {
        Editor::launch(options, Value::Null)
    }

    fn launch(options: Value, folders: Value) -> Editor {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut editor = Editor { child, input, output, next: 0, diagnostics: Vec::new(), tests: Vec::new() };
        let capabilities = json!({ "general": { "positionEncodings": ["utf-8"] } });
        let initialized = editor.request(
            "initialize",
            json!({ "capabilities": capabilities, "initializationOptions": options, "workspaceFolders": folders }),
        );
        assert_eq!(initialized["capabilities"]["positionEncoding"], "utf-8");
        editor.notify("initialized", json!({}));
        editor
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.input, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.input.flush().unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut length = 0;
        loop {
            let mut line = String::new();
            self.output.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        self.output.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.receive();
            if message["id"] == id {
                return message.get("result").cloned().unwrap_or_else(|| message["error"].clone());
            }
            if message["method"] == "tondo/tests" {
                self.tests.push(message["params"].clone());
            }
            if message["method"] == "textDocument/publishDiagnostics" {
                let uri = message["params"]["uri"].as_str().unwrap().to_string();
                self.diagnostics.retain(|(u, _)| *u != uri);
                self.diagnostics.push((uri, message["params"]["diagnostics"].clone()));
            }
        }
    }

    fn open(&mut self, path: &Path) -> String {
        let uri = format!("file://{}", path.display());
        let text = std::fs::read_to_string(path).unwrap();
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "tondo", "version": 1, "text": text } }),
        );
        uri
    }

    fn at(&mut self, method: &str, uri: &str, text: &str, line: &str, word: &str, extra: Value) -> Value {
        let (number, found) = text.lines().enumerate().find(|(_, l)| l.contains(line)).expect(line);
        let character = found.find(line).unwrap() + line.find(word).expect(word);
        let mut params =
            json!({ "textDocument": { "uri": uri }, "position": { "line": number, "character": character } });
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        self.request(method, params)
    }

    fn stop(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        assert!(self.child.wait().unwrap().success());
    }
}

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir.canonicalize().unwrap()
}

const MODELS: &str = r#"/// A person who uses the app.
pub type User = {
    /// What they are called.
    name: String
    age: Int
}

pub enum Role {
    Admin
    Member(Int)
}

/// Says who a user is.
pub fn User.describe(self): String {
    "{self.name} ({self.age})"
}

pub fn make(name: String, age: Int = 30): User {
    User { name, age }
}
"#;

const MAIN: &str = r#"import std.console
import app.models

const LIMIT: Int = 3

fn greet(user: models.User, times: Int): String {
    var text = ""
    for _ in 0..times {
        text = "{text}{user.describe()}"
    }
    text
}

fn main() {
    let ada = models.make("Ada", age: 36)
    match models.Role.Member(LIMIT) {
        models.Role.Admin => console.println("admin")
        models.Role.Member(level) => console.println("{level}")
    }
    console.println(greet(ada, LIMIT))
    console.println(ada.name)
}
"#;

#[test]
fn hover_definitions_references_and_rename() {
    let dir = project(
        "lsp-navigation",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/models/models.to", MODELS), ("src/main.to", MAIN)],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let models = format!("file://{}", dir.join("src/models/models.to").display());
    let hover = |editor: &mut Editor, line: &str, word: &str| {
        let result = editor.at("textDocument/hover", &uri, MAIN, line, word, json!({}));
        result["contents"]["value"].as_str().unwrap_or_default().to_string()
    };
    assert_eq!(hover(&mut editor, "console.println(ada.name)", "ada"), "```tondo\nlet ada: models.User\n```");
    assert_eq!(
        hover(&mut editor, "console.println(ada.name)", "name"),
        "```tondo\nname: String\n```\n\nWhat they are called."
    );
    assert_eq!(
        hover(&mut editor, "user.describe()", "describe"),
        "```tondo\npub fn User.describe(self): String\n```\n\nSays who a user is."
    );
    assert_eq!(hover(&mut editor, "models.Role.Admin =>", "Admin"), "```tondo\nRole.Admin\n```");
    assert_eq!(hover(&mut editor, "age: 36", "age"), "```tondo\nage: Int\n```");
    assert_eq!(hover(&mut editor, "greet(ada, LIMIT))", "LIMIT"), "```tondo\nconst LIMIT: Int = 3\n```");

    let definition = editor.at("textDocument/definition", &uri, MAIN, "user: models.User", "User", json!({}));
    assert_eq!(definition["uri"], models);
    assert_eq!(definition["range"]["start"], json!({ "line": 1, "character": 9 }));

    // The field is written in the literal's shorthand, its access and here.
    let context = json!({ "context": { "includeDeclaration": true } });
    let references = editor.at("textDocument/references", &uri, MAIN, "console.println(ada.name)", "name", context);
    assert_eq!(references.as_array().unwrap().len(), 4, "{references}");

    let prepared = editor.at("textDocument/prepareRename", &uri, MAIN, "console.println(ada.name)", "name", json!({}));
    assert_eq!(prepared["placeholder"], "name");
    let renamed = editor.at(
        "textDocument/rename",
        &uri,
        MAIN,
        "console.println(ada.name)",
        "name",
        json!({ "newName": "fullName" }),
    );
    let edits = &renamed["changes"][&models];
    let texts: Vec<&str> = edits.as_array().unwrap().iter().map(|e| e["newText"].as_str().unwrap()).collect();
    // `User { name, age }` keeps its binding: `fullName: name`.
    assert!(texts.contains(&"fullName: name") && texts.contains(&"fullName"), "{renamed}");
    let refused =
        editor.at("textDocument/rename", &uri, MAIN, "console.println(ada.name)", "println", json!({ "newName": "x" }));
    assert!(refused.is_null() || refused["message"].is_string(), "{refused}");

    let symbols = editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    let names: Vec<&str> = symbols.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["LIMIT", "greet", "main"]);
    editor.stop();
}

#[test]
fn diagnostics_follow_the_edits() {
    let dir = project("lsp-diagnostics", &[("main.to", "fn main() {\n    let x: Int = \"text\"\n    _ = x\n}\n")]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    let formatted = editor.request("textDocument/formatting", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(formatted, json!([]));
    let found = |editor: &Editor| editor.diagnostics.iter().find(|(u, _)| *u == uri).map(|(_, d)| d.clone());
    // A request after a change sees it; the diagnostics come first.
    editor.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } }),
    );
    let diagnostics = found(&editor).expect("diagnostics");
    assert_eq!(diagnostics[0]["code"], "E1102");
    assert_eq!(diagnostics[0]["range"]["start"], json!({ "line": 1, "character": 17 }));
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": "fn main() {\n    let x: Int = 1\n    _ = x\n}\n" }] }),
    );
    editor.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } }),
    );
    assert_eq!(found(&editor), Some(json!([])));
    editor.stop();
}

#[test]
fn completion_and_highlighting() {
    let dir = project(
        "lsp-completion",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/models/models.to", MODELS), ("src/main.to", MAIN)],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let complete = |editor: &mut Editor, typed: &str| -> Vec<String> {
        // The line is being written: it does not parse yet.
        let text =
            MAIN.replace("    console.println(ada.name)\n", &format!("    console.println(ada.name)\n    {typed}\n"));
        editor.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
        let line = text.lines().position(|l| l == format!("    {typed}")).unwrap();
        let position = json!({ "line": line, "character": 4 + typed.len() });
        let result =
            editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
        result["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(complete(&mut editor, "ada."), ["name", "age", "describe"]);
    assert_eq!(complete(&mut editor, "models."), ["User", "Role", "make"]);
    // A standard module, a type's functions and an enum's variants.
    let console = complete(&mut editor, "console.");
    assert!(console.contains(&"println".to_string()) && console.contains(&"readLine".to_string()), "{console:?}");
    assert_eq!(complete(&mut editor, "models.Role."), ["Admin", "Member"]);
    let strings = complete(&mut editor, "String.");
    assert!(strings.contains(&"fromBytes".to_string()) && !strings.contains(&"length".to_string()), "{strings:?}");
    let strings = complete(&mut editor, "\"text\".");
    assert!(strings.contains(&"length".to_string()) && strings.contains(&"split".to_string()), "{strings:?}");
    let names = complete(&mut editor, "gr");
    assert!(names.contains(&"greet".to_string()) && names.contains(&"ada".to_string()), "{names:?}");

    let tokens = editor.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    let data: Vec<u64> = tokens["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    // `import` first: a keyword at the start of the file.
    assert_eq!(data[..5], [0, 0, 6, 11, 0]);
    editor.stop();
}

#[test]
fn repairs_as_code_actions() {
    let dir = project("lsp-repairs", &[("main.to", "fn main() {\n    let x: Int? = None\n    _ = x\n}\n")]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    let actions = |editor: &mut Editor, character: u64| {
        let at = json!({ "line": 1, "character": character });
        let params = json!({ "textDocument": { "uri": uri }, "range": { "start": at, "end": at }, "context": { "diagnostics": [] } });
        editor.request("textDocument/codeAction", params)
    };
    let titles = |actions: &Value| -> Vec<String> {
        actions.as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap().to_string()).collect()
    };
    // `none` needs a type that context may not give: a decision.
    let found = actions(&mut editor, 18);
    assert_eq!(titles(&found), ["write `none`"], "{found}");
    assert_eq!(found[0]["kind"], "quickfix");
    assert_eq!(found[0]["isPreferred"], false);
    assert_eq!(found[0]["edit"]["changes"][&uri][0]["newText"], "none");

    // A `;` is an error of its own, which hides those that would follow it.
    let text = "fn main() {\n    let x: Int? = None;\n    _ = x\n}\n";
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let found = actions(&mut editor, 22);
    assert_eq!(titles(&found), ["remove the `;`", "Apply every safe repair"], "{found}");
    editor.stop();
}

#[test]
fn highlights_symbols_signatures_hints() {
    let dir = project(
        "lsp-assist",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/models/models.to", MODELS), ("src/main.to", MAIN)],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));

    let highlights =
        editor.at("textDocument/documentHighlight", &uri, MAIN, "console.println(ada.name)", "ada", json!({}));
    let kinds: Vec<u64> = highlights.as_array().unwrap().iter().map(|h| h["kind"].as_u64().unwrap()).collect();
    assert_eq!(kinds, [3, 2, 2], "{highlights}");

    let symbols = editor.request("workspace/symbol", json!({ "query": "gre" }));
    let names: Vec<&str> = symbols.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["greet"]);

    // The runtime's functions show what `tondo api` says of them.
    let hover = editor.at("textDocument/hover", &uri, MAIN, "console.println(ada.name)", "println", json!({}));
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.starts_with("```tondo\npub fn println(text: String)"), "{value}");

    let signature = |editor: &mut Editor, typed: &str| -> Value {
        let text =
            MAIN.replace("    console.println(ada.name)\n", &format!("    console.println(ada.name)\n    {typed}\n"));
        editor.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
        let line = text.lines().position(|l| l == format!("    {typed}")).unwrap();
        let position = json!({ "line": line, "character": 4 + typed.len() });
        editor.request("textDocument/signatureHelp", json!({ "textDocument": { "uri": uri }, "position": position }))
    };
    let help = signature(&mut editor, "greet(ada, ");
    assert_eq!(help["signatures"][0]["label"], "fn greet(user: models.User, times: Int): String");
    assert_eq!(help["activeParameter"], 1);
    let help = signature(&mut editor, "models.make(\"Grace\", ");
    assert_eq!(help["signatures"][0]["parameters"][1]["label"], "age: Int = 30");
    let help = signature(&mut editor, "console.println(");
    assert_eq!(help["signatures"][0]["label"], "fn println(text: String)");

    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": MAIN }] }),
    );
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 30, "character": 0 } });
    let hints = editor.request("textDocument/inlayHint", json!({ "textDocument": { "uri": uri }, "range": range }));
    let labels: Vec<&str> = hints.as_array().unwrap().iter().map(|h| h["label"].as_str().unwrap()).collect();
    // The types of bindings declared without one, then the parameters
    // arguments go to: `make("Ada", age: 36)` names its second.
    assert_eq!(labels, [": String", ": models.User", ": Int", "name:", "user:", "times:"], "{hints}");
    editor.stop();
}

/// With `tests` asked for, the server sends a project's suites and tests
/// as its files change; Run and Debug lenses sit over `main` or a script's
/// first statement, in the file that holds it.
#[test]
fn tells_tests_and_lenses() {
    let main = "import std.console\n\nfn main() {\n    console.println(\"hi\")\n}\n";
    let tests = "import std.testing\n\ntest first {\n    assert(1 == 1)\n}\n\ntest measured {\n    testing.bench(\"x\", () { 1 })\n}\n\nsuite group {\n    test inner {}\n}\n";
    let dir = project(
        "lsp-tests",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", main), ("src/main_test.to", tests)],
    );
    let mut editor = Editor::start_with(json!({ "tests": true }));
    let tested = editor.open(&dir.join("src/main_test.to"));
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": tested } }));
    assert_eq!(lenses, json!([]));
    // A request about the file has its project analyzed, as a pause does.
    editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": tested } }));
    let sent = editor.tests.last().cloned().expect("the project's tests");
    assert_eq!(sent["root"], format!("file://{}", dir.display()));
    let ids: Vec<(&str, &Value, bool, bool)> = sent["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["id"].as_str().unwrap(), &t["parent"], t["suite"].as_bool().unwrap(), t["bench"] == true))
        .collect();
    assert_eq!(
        ids,
        [
            ("app::unit::app::first", &Value::Null, false, false),
            ("app::unit::app::measured", &Value::Null, false, true),
            ("app::unit::app::group", &Value::Null, true, false),
            ("app::unit::app::group::inner", &json!("app::unit::app::group"), false, false),
        ]
    );
    assert_eq!(sent["tests"][3]["name"], "inner");
    assert_eq!(sent["tests"][3]["location"]["uri"], tested);
    assert_eq!(sent["tests"][3]["location"]["range"]["start"], json!({ "line": 11, "character": 9 }));

    // Over main, in the file that holds it.
    let opened = editor.open(&dir.join("src/main.to"));
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": opened } }));
    let titles: Vec<(&str, &str)> = lenses
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["command"]["title"].as_str().unwrap(), l["command"]["command"].as_str().unwrap()))
        .collect();
    assert_eq!(
        titles,
        [
            ("Run", "tondo.run"),
            ("Debug", "tondo.debug"),
            ("Debug Live", "tondo.debugLive"),
            ("Profile", "tondo.profile")
        ]
    );
    assert_eq!(lenses[0]["range"]["start"], json!({ "line": 2, "character": 3 }));
    assert_eq!(lenses[0]["command"]["arguments"], json!([opened]));

    // A test written, not saved, is sent; nothing new sends nothing.
    let count = editor.tests.len();
    let edited = format!("{tests}\ntest second {{}}\n");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": tested, "version": 2 }, "contentChanges": [{ "text": edited }] }),
    );
    editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": tested } }));
    assert_eq!(editor.tests.len(), count + 1);
    assert_eq!(editor.tests.last().unwrap()["tests"][4]["id"], "app::unit::app::second");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": tested, "version": 3 }, "contentChanges": [{ "text": edited }] }),
    );
    editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": tested } }));
    assert_eq!(editor.tests.len(), count + 1);

    // A script's first statement, in a file on its own.
    let script = project("lsp-script", &[("hello.to", "import std.console\n\nconsole.println(\"hi\")\n")]);
    let opened = editor.open(&script.join("hello.to"));
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": opened } }));
    assert_eq!(lenses[1]["range"]["start"], json!({ "line": 2, "character": 0 }));
    editor.stop();

    // Without asking, no tests are sent.
    let mut editor = Editor::start();
    let tested = editor.open(&dir.join("src/main_test.to"));
    editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": tested } }));
    assert!(editor.tests.is_empty());
    editor.stop();
}

const SHAPED: &str = "import std.console
import app.shapes

trait Named {
    fn name(self): String
}

type Box = { label: String, size: Int }

impl Named for Box {
    fn name(self): String {
        self.label
    }
}

fn area(width: Int, height: Int): Int {
    width * height
}

fn describe(box: Box): String {
    let total = area(box.size, 2)
    let named = Named.name(box)
    \"{named} {total}\"
}

fn main() {
    let box = Box { label: \"a\", size: 3 }
    console.println(describe(box))
    console.println(\"{shapes.sides()}\")
}
";

/// Calls, implementations, types, the structure of a file, hints of
/// parameters, imports added, and a folder renamed.
#[test]
fn navigates_calls_types_and_structure() {
    let dir = project(
        "lsp-structure",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", SHAPED),
            ("src/shapes/shapes.to", "pub fn sides(): Int {\n    3\n}\n"),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let at = |line: usize, character: usize| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });

    // Who calls `area`, and what `describe` calls.
    let prepared = editor.request("textDocument/prepareCallHierarchy", at(15, 4));
    assert_eq!(prepared[0]["name"], "area");
    let incoming = editor.request("callHierarchy/incomingCalls", json!({ "item": prepared[0] }));
    assert_eq!(incoming[0]["from"]["name"], "describe");
    let describe = editor.request("textDocument/prepareCallHierarchy", at(19, 4));
    let outgoing = editor.request("callHierarchy/outgoingCalls", json!({ "item": describe[0] }));
    let called: Vec<&str> = outgoing.as_array().unwrap().iter().map(|c| c["to"]["name"].as_str().unwrap()).collect();
    assert!(called.contains(&"area") && called.contains(&"name"), "{called:?}");

    // A trait's implementations, a method's, and a binding's type.
    let impls = editor.request("textDocument/implementation", at(3, 7));
    assert_eq!(impls[0]["range"]["start"]["line"], 9);
    let methods = editor.request("textDocument/implementation", at(4, 8));
    assert_eq!(methods[0]["range"]["start"], json!({ "line": 10, "character": 7 }));
    let typed = editor.request("textDocument/typeDefinition", at(19, 12));
    assert_eq!(typed["range"]["start"], json!({ "line": 7, "character": 5 }));
    let boxed = editor.request("textDocument/prepareTypeHierarchy", at(7, 6));
    let supertypes = editor.request("typeHierarchy/supertypes", json!({ "item": boxed[0] }));
    assert_eq!(supertypes[0]["name"], "Named");
    let named = editor.request("textDocument/prepareTypeHierarchy", at(3, 7));
    let subtypes = editor.request("typeHierarchy/subtypes", json!({ "item": named[0] }));
    assert_eq!(subtypes[0]["name"], "Box");

    // The parameters arguments go to, unless named so already.
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 28, "character": 0 } });
    let hints = editor.request("textDocument/inlayHint", json!({ "textDocument": { "uri": uri }, "range": range }));
    let labels: Vec<(u64, &str)> = hints
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["kind"] == 2)
        .map(|h| (h["position"]["line"].as_u64().unwrap(), h["label"].as_str().unwrap()))
        .collect();
    assert_eq!(labels, [(20, "width:"), (20, "height:")]);

    // Folds, and selections growing from a name.
    let folds = editor.request("textDocument/foldingRange", json!({ "textDocument": { "uri": uri } }));
    let folds: Vec<(u64, u64, Option<&str>)> = folds
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["startLine"].as_u64().unwrap(), f["endLine"].as_u64().unwrap(), f["kind"].as_str()))
        .collect();
    assert!(folds.contains(&(0, 1, Some("imports"))) && folds.contains(&(9, 12, None)), "{folds:?}");
    let selected = editor.request(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": uri }, "positions": [{ "line": 20, "character": 23 }] }),
    );
    assert_eq!(selected[0]["range"]["start"], json!({ "line": 20, "character": 21 }));
    assert_eq!(selected[0]["parent"]["range"]["start"], json!({ "line": 20, "character": 21 }));
    assert_eq!(selected[0]["parent"]["range"]["end"], json!({ "line": 20, "character": 32 }));

    // A module not imported yet, completed with its import.
    let completed = editor.request("textDocument/completion", at(26, 4));
    let json_module = completed["items"].as_array().unwrap().iter().find(|i| i["label"] == "json").unwrap();
    assert_eq!(json_module["additionalTextEdits"][0]["newText"], "import std.json\n");
    assert_eq!(json_module["additionalTextEdits"][0]["range"]["start"], json!({ "line": 2, "character": 0 }));

    // Only the lines of a range are formatted.
    let messy = SHAPED
        .replace("fn area(width: Int, height: Int): Int {", "fn area(width:Int,height:Int):Int {")
        .replace("fn main() {", "fn main()   {");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": messy }] }),
    );
    let range = json!({ "start": { "line": 15, "character": 0 }, "end": { "line": 16, "character": 0 } });
    let edits = editor.request(
        "textDocument/rangeFormatting",
        json!({ "textDocument": { "uri": uri }, "range": range, "options": {} }),
    );
    assert_eq!(edits.as_array().unwrap().len(), 1, "{edits}");
    assert_eq!(edits[0]["newText"], "fn area(width: Int, height: Int): Int {\n");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": SHAPED }] }),
    );

    // The folder of a module renamed: its import and its uses follow.
    let renamed = editor.request(
        "workspace/willRenameFiles",
        json!({ "files": [{ "oldUri": format!("file://{}", dir.join("src/shapes").display()), "newUri": format!("file://{}", dir.join("src/figures").display()) }] }),
    );
    let mut texts: Vec<(u64, &str)> = renamed["changes"][&uri]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["range"]["start"]["line"].as_u64().unwrap(), e["newText"].as_str().unwrap()))
        .collect();
    texts.sort();
    assert_eq!(texts, [(1, "app.figures"), (28, "figures")]);
    editor.stop();
}

/// The projects of the editor's folders show their problems when it
/// starts, before their files are opened; a name a module of the program
/// has offers its import.
#[test]
fn checks_the_workspace_and_imports_modules() {
    let dir = project(
        "lsp-workspace",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", "fn main() {\n    let x = shapes.sides()\n}\n"),
            ("src/shapes/shapes.to", "pub fn sides(): Int {\n    3\n}\n"),
        ],
    );
    let mut editor = Editor::start_with_folders(&dir);
    editor.notify("initialized", json!({}));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    editor.request("workspace/symbol", json!({ "query": "" }));
    let uri = format!("file://{}", dir.join("src/main.to").display());
    let (_, diagnostics) =
        editor.diagnostics.iter().find(|(u, _)| *u == uri).cloned().expect("diagnostics of a file not opened");
    let unknown = diagnostics.as_array().unwrap().iter().find(|d| d["code"] == "E1001").unwrap();
    assert!(unknown["message"].as_str().unwrap().contains("add `import app.shapes`"), "{diagnostics}");
    editor.open(&dir.join("src/main.to"));
    let range = json!({ "start": { "line": 1, "character": 12 }, "end": { "line": 1, "character": 12 } });
    let actions = editor.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [] } }),
    );
    let import = actions.as_array().unwrap().iter().find(|a| a["title"] == "add `import app.shapes`").unwrap();
    assert_eq!(import["edit"]["changes"][&uri][0]["newText"], "import app.shapes\n");
    editor.stop();
}
