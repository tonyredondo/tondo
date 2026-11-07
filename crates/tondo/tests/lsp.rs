//! `tondo lsp` (toolchain 7.4) as an editor uses it, over standard input
//! and output.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// What an editor as VS Code says it takes, of what changes the answers.
fn everything() -> Value {
    json!({
        "textDocument": {
            "hover": { "contentFormat": ["markdown"] },
            "signatureHelp": { "signatureInformation": { "documentationFormat": ["markdown"] } },
            "completion": { "completionItem": {
                "snippetSupport": true,
                "labelDetailsSupport": true,
                "documentationFormat": ["markdown"],
            } },
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
            "codeAction": { "codeActionLiteralSupport": {} },
        },
    })
}

struct Editor {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    /// The diagnostics last published for each file.
    diagnostics: Vec<(String, Value)>,
    /// The tests of projects sent, in order (`tondo/tests`).
    tests: Vec<Value>,
    /// Each publication of diagnostics: the file, its version, what it said.
    versions: Vec<(String, u64, Value)>,
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
        Editor::launch_with(everything(), options, folders, "off")
    }

    fn launch_with(mut capabilities: Value, options: Value, folders: Value, trace: &str) -> Editor {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("lsp")
            .env("TONDO_TEST_LSP_PANIC", "1")
            .stderr(Stdio::null())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut editor =
            Editor { child, input, output, next: 0, diagnostics: Vec::new(), tests: Vec::new(), versions: Vec::new() };
        capabilities["general"] = json!({ "positionEncodings": ["utf-8"] });
        let initialized = editor.request(
            "initialize",
            json!({ "capabilities": capabilities, "initializationOptions": options, "workspaceFolders": folders,
                "trace": trace }),
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

    /// The next request or notification of a method, past all else.
    fn receive_method(&mut self, method: &str) -> Value {
        loop {
            let message = self.receive();
            if message["method"] == method {
                return message;
            }
        }
    }

    /// The next diagnostics published of a file.
    fn receive_diagnostics(&mut self, uri: &str) -> Value {
        loop {
            let message = self.receive_method("textDocument/publishDiagnostics");
            if message["params"]["uri"] == uri {
                return message["params"]["diagnostics"].clone();
            }
        }
    }

    /// The version and the diagnostics last published of a file.
    fn published(&mut self, uri: &str) -> (u64, Value) {
        let last = self.versions.iter().rev().find(|(u, ..)| u == uri).expect("no diagnostics");
        (last.1, last.2.clone())
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
                self.diagnostics.push((uri.clone(), message["params"]["diagnostics"].clone()));
                let version = message["params"]["version"].as_u64().unwrap_or_default();
                self.versions.push((uri, version, message["params"]["diagnostics"].clone()));
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

const CONTEXTS: &str = r#"import std.console

context Request = {
    /// How deep the call is.
    depth: Int = 0
}

fn show() {
    console.println("{Request.depth}")
}

fn main() {
    for depth in 1..3 {
        with Request { depth }
        show()
        console.println("{Request.depth}")
    }
}
"#;

#[test]
fn hover_tells_where_a_context_got_its_value() {
    let dir = project("lsp-contexts", &[("main.to", CONTEXTS)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    let mut hover = |line: &str| {
        let result = editor.at("textDocument/hover", &uri, CONTEXTS, line, "depth", json!({}));
        result["contents"]["value"].as_str().unwrap_or_default().to_string()
    };
    // The first line that has it is in `show`; the second, after the `with`.
    let read = "    console.println(\"{Request.depth}\")";
    assert_eq!(
        hover(read),
        "```tondo\ndepth: Int\n```\n\nHow deep the call is.\n\nHere its value is the one it has where this \
         function is called, or in the task that started this one."
    );
    assert_eq!(
        hover(&format!("    {read}")),
        "```tondo\ndepth: Int\n```\n\nHow deep the call is.\n\nHere its value is the one `with` gave it at line 14."
    );
    // `context` is a word of the language before `Request =`.
    let tokens = editor.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    let data: Vec<u64> = tokens["data"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap()).collect();
    let mut line = 0;
    let found: Vec<(u64, u64)> = data
        .chunks(5)
        .filter_map(|t| {
            line += t[0];
            (line == 2).then_some((t[1], t[3]))
        })
        .collect();
    assert_eq!(found[0], (0, 11), "{tokens}");
    editor.stop();
}

const NUMBERS: &str = "import std.console\n\nfn main() {\n    let word: UInt16 = 0x1234\n    console.println(\"{word.reverseBytes()}\")\n}\n";

#[test]
fn hover_shows_the_operations_of_numbers() {
    let dir = project("lsp-numbers", &[("main.to", NUMBERS)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    let hover = editor.at("textDocument/hover", &uri, NUMBERS, "word.reverseBytes()", "reverseBytes", json!({}));
    let value = hover["contents"]["value"].as_str().unwrap_or_default();
    assert!(value.starts_with("```tondo\npub fn UInt16.reverseBytes(self): UInt16\n```"), "{hover}");
    assert!(value.contains("big-endian"), "{value}");
    editor.stop();
}

const BINDING: &str = "/// What a value must be.\npub annotation Check = { required: Bool = false, min: Int? = none }\n\npub type Plain = { x: Int }\n";

const ANNOTATED: &str =
    "import app.binding\n\npub type User = {\n    @binding.Check(min: 3)\n    name: String\n}\n\nfn main() {}\n";

#[test]
fn completes_and_explains_annotations() {
    let dir = project(
        "lsp-annotations",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/binding/binding.to", BINDING),
            ("src/main.to", ANNOTATED),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let hover = editor.at("textDocument/hover", &uri, ANNOTATED, "@binding.Check", "Check", json!({}));
    let value = hover["contents"]["value"].as_str().unwrap_or_default();
    assert_eq!(
        value,
        "```tondo\npub annotation Check = { required: Bool = false, min: Int? = none }\n```\n\nWhat a value must be."
    );
    let definition = editor.at("textDocument/definition", &uri, ANNOTATED, "@binding.Check", "Check", json!({}));
    assert!(definition["uri"].as_str().unwrap().ends_with("src/binding/binding.to"), "{definition}");
    // After `@`, the annotations, not other types; in the arguments, the
    // fields not written yet.
    let complete = |editor: &mut Editor, typed: &str| -> Vec<String> {
        let text = ANNOTATED.replace("    name: String\n", &format!("    name: String\n    {typed}\n    age: Int\n"));
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
    assert_eq!(complete(&mut editor, "@"), ["binding"]);
    assert_eq!(complete(&mut editor, "@binding."), ["Check"]);
    assert_eq!(complete(&mut editor, "@binding.Check(min: 1, "), ["required"]);
    editor.stop();
}

const EMBEDDING: &str = "import std.embed\n\nconst Page: String = embed.text(\"static/index.html\")\n\nfn main() {}\n";

#[test]
fn goes_to_and_completes_embedded_paths() {
    let dir = project(
        "lsp-embed",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", EMBEDDING),
            ("src/static/index.html", "<h1>home</h1>\n"),
            ("src/static/css/site.css", "body {}\n"),
            ("src/static/.env", "secret\n"),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let found = editor.at("textDocument/definition", &uri, EMBEDDING, "\"static/index.html\"", "index", json!({}));
    assert_eq!(found["uri"], format!("file://{}", dir.join("src/static/index.html").display()), "{found}");
    // In the path, what the directory written so far has, but hidden files.
    let text = EMBEDDING.replace("\"static/index.html\")", "\"static/");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let line = text.lines().position(|l| l.contains("static/")).unwrap();
    let character = text.lines().nth(line).unwrap().len();
    let position = json!({ "line": line, "character": character });
    let result =
        editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
    let labels: Vec<&str> = result["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["css/", "index.html"]);
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
        // The members, without the templates around a value (`.match`...).
        let members = result["items"].as_array().unwrap().iter().filter(|i| i["kind"] != 15);
        members.map(|i| i["label"].as_str().unwrap().to_string()).collect()
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
    // In an import, the modules: `std` and the project's first, then the
    // standard ones, each with what it is for.
    let imports = |editor: &mut Editor, typed: &str| -> Vec<Value> {
        let text = format!("{typed}\n{MAIN}");
        editor.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
        let position = json!({ "line": 0, "character": typed.len() });
        let result =
            editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
        result["items"].as_array().unwrap().clone()
    };
    let labels =
        |items: &[Value]| -> Vec<String> { items.iter().map(|i| i["label"].as_str().unwrap().to_string()).collect() };
    let roots = labels(&imports(&mut editor, "import "));
    assert!(roots.contains(&"std".to_string()) && roots.contains(&"app".to_string()), "{roots:?}");
    let standard = imports(&mut editor, "import std.");
    let http = standard.iter().find(|i| i["label"] == "http").expect("std.http");
    assert!(http["labelDetails"]["description"].as_str().unwrap().starts_with("an HTTP client"), "{http}");
    assert!(!labels(&standard).contains(&"internal".to_string()));
    assert_eq!(labels(&imports(&mut editor, "import app.")), ["models"]);

    let tokens = editor.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    let data: Vec<u64> = tokens["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    // `import` first: a keyword at the start of the file.
    assert_eq!(data[..5], [0, 0, 6, 11, 0]);
    editor.stop();
}

/// A route's handler, a closure of several lines, is an expression the
/// checker names: no use of a name covers it, so its tokens stay its own
/// and do not overlap.
#[test]
fn highlights_a_route_handler() {
    let routed = "import std.http\n\nfn main(): !http.HttpError {\n    var app = http.App()\n    app.get(\n        \"/fail\",\n        (_) {\n            fail http.Problem.notFound(\"none\")\n        },\n    )\n    _ = app.start()?\n}\n";
    let dir = project("lsp-routes", &[("main.to", routed)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    let tokens = editor.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    let data: Vec<u64> = tokens["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    // Each token by its line and column, from the relative ones.
    let (mut line, mut column, mut at) = (0, 0, Vec::new());
    for token in data.chunks(5) {
        line += token[0];
        column = if token[0] == 0 { column + token[1] } else { token[1] };
        at.push((line, column, token[2], token[3]));
    }
    // The texts of the route and of the handler's body are strings.
    assert!(at.contains(&(5, 8, 7, 13)) && at.contains(&(7, 39, 6, 13)), "{at:?}");
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

    // The project's declarations, then the standard library's, each with
    // its module, where the editor opens it.
    let symbols = editor.request("workspace/symbol", json!({ "query": "gre" }));
    let symbols = symbols.as_array().unwrap();
    assert_eq!(symbols[0]["name"], "greet");
    assert!(symbols[1..].iter().all(|s| s["containerName"].as_str().is_some()), "{symbols:?}");
    let found = |editor: &mut Editor, query: &str| -> Value {
        let symbols = editor.request("workspace/symbol", json!({ "query": query }));
        symbols.as_array().unwrap().iter().find(|s| s["name"] == query).cloned().unwrap_or_default()
    };
    let encode = found(&mut editor, "base64Encode");
    assert_eq!(encode["containerName"], "std.encoding");
    assert!(encode["location"]["uri"].as_str().unwrap().ends_with("/encoding.to"), "{encode}");
    let println = found(&mut editor, "println");
    assert_eq!(println["containerName"], "std.console");
    assert!(println["location"]["uri"].as_str().unwrap().ends_with("/api/console.to"), "{println}");
    // A standard module's import says what it is for.
    let hover = editor.at("textDocument/hover", &uri, MAIN, "import std.console", "console", json!({}));
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.starts_with("```tondo\nmodule std.console\n```\n\n"), "{value}");

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
    // On save, the imports the file lacks and not those it does not use.
    let organize = editor.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range,
            "context": { "diagnostics": [], "only": ["source.organizeImports"] } }),
    );
    let [organize] = organize.as_array().unwrap().as_slice() else { panic!("{organize}") };
    assert_eq!(organize["kind"], "source.organizeImports");
    assert_eq!(organize["edit"]["changes"][&uri][0]["newText"], "import app.shapes\n");
    editor.stop();
}

#[test]
fn reads_the_files_of_each_system() {
    // The other system than this machine's, whose file has an error.
    let (here, other) = if cfg!(target_os = "macos") { ("macos", "linux") } else { ("linux", "macos") };
    let shown = |system: &str| if system == "macos" { "macOS" } else { "Linux" };
    let main = "import std.console\n\nfn main() {\n    console.println(describe())\n}\n";
    let good = format!("fn describe(): String {{\n    \"{}\"\n}}\n", shown(here));
    let bad = "fn describe(): String {\n    1\n}\n".to_string();
    let test = |system: &str| format!("test describes{} {{\n    assert(describe().length() > 0)\n}}\n", shown(system));
    let dir = project(
        "lsp-systems",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", main),
            (&format!("src/describe_{here}.to"), &good),
            (&format!("src/describe_{other}.to"), &bad),
            (&format!("src/describe_{here}_test.to"), &test(here)),
            (&format!("src/describe_{other}_test.to"), &test(other)),
        ],
    );
    let mut editor = Editor::start_with(json!({ "tests": true }));
    let uri = editor.open(&dir.join("src/main.to"));
    // The other system's error, with its name.
    let hover = editor.at("textDocument/hover", &uri, main, "describe()", "describe", json!({}));
    let theirs = format!("file://{}", dir.join(format!("src/describe_{other}.to")).display());
    let found = editor.diagnostics.iter().find(|(u, _)| *u == theirs).map(|(_, d)| d.clone()).expect("diagnostics");
    assert_eq!(found[0]["message"], format!("on {}: expected `String`, found `Int`", shown(other)), "{found}");
    // Where each system declares it.
    let value = hover["contents"]["value"].as_str().unwrap();
    let mut places = [(here, shown(here)), (other, shown(other))];
    places.sort_by_key(|&(_, name)| name);
    let expected: Vec<String> =
        places.iter().map(|(system, name)| format!("for {name} in `src/describe_{system}.to`")).collect();
    assert!(value.ends_with(&format!("One declaration {}.", expected.join(", "))), "{value}");
    let definitions = editor.at("textDocument/definition", &uri, main, "describe()", "describe", json!({}));
    let mut uris: Vec<&str> = definitions.as_array().unwrap().iter().map(|d| d["uri"].as_str().unwrap()).collect();
    uris.sort();
    let mut wanted = [format!("file://{}", dir.join(format!("src/describe_{here}.to")).display()), theirs.clone()];
    wanted.sort();
    assert_eq!(uris, wanted);
    // The other system's file is read as that system reads it.
    let opened = editor.open(&dir.join(format!("src/describe_{other}.to")));
    let hover = editor.at("textDocument/hover", &opened, &bad, "fn describe", "describe", json!({}));
    assert!(hover["contents"]["value"].as_str().unwrap().starts_with("```tondo\nfn describe(): String"), "{hover}");
    // Its tests too, which this machine does not run, once it builds.
    let fixed = format!("fn describe(): String {{\n    \"{}\"\n}}\n", shown(other));
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": opened, "version": 2 }, "contentChanges": [{ "text": fixed }] }),
    );
    editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": opened } }));
    let sent = editor.tests.last().cloned().expect("the project's tests");
    let systems: Vec<(&str, &Value)> =
        sent["tests"].as_array().unwrap().iter().map(|t| (t["name"].as_str().unwrap(), &t["system"])).collect();
    let mut expected = vec![
        (format!("describes{}", shown(here)), Value::Null),
        (format!("describes{}", shown(other)), json!(shown(other))),
    ];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    let mut systems: Vec<(String, Value)> = systems.into_iter().map(|(n, s)| (n.to_string(), s.clone())).collect();
    systems.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(systems, expected);
    editor.stop();
}

/// What helps write and read Tondo in an editor beyond navigation: the
/// keywords as snippets, a call's parameters by name and a record's
/// fields, the type of a closure's parameter, unused code faded, what a
/// keyword does, the values a debugger shows beside the lines, the tokens
/// of some lines, and an arm for what a `match` misses.
#[test]
fn writing_helps() {
    let dir = project(
        "lsp-writing",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/models/models.to", MODELS), ("src/main.to", MAIN)],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let with_line = |editor: &mut Editor, typed: &str| -> String {
        let text =
            MAIN.replace("    console.println(ada.name)\n", &format!("    console.println(ada.name)\n    {typed}\n"));
        editor.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
        text
    };
    let complete = |editor: &mut Editor, typed: &str| -> Vec<Value> {
        let text = with_line(editor, typed);
        let line = text.lines().position(|l| l == format!("    {typed}")).unwrap();
        let position = json!({ "line": line, "character": 4 + typed.len() });
        let result =
            editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
        result["items"].as_array().unwrap().clone()
    };
    let labels =
        |items: &[Value]| -> Vec<String> { items.iter().map(|i| i["label"].as_str().unwrap().to_string()).collect() };
    let keyword = complete(&mut editor, "ma").into_iter().find(|i| i["label"] == "match").unwrap();
    assert_eq!(keyword["insertTextFormat"], 2);
    assert!(keyword["insertText"].as_str().unwrap().starts_with("match ${1:value} {"), "{keyword}");
    assert_eq!(labels(&complete(&mut editor, "_ = greet(ada, ")[..1]), ["times:"]);
    assert_eq!(labels(&complete(&mut editor, "_ = models.make(\"Al\", ")[..1]), ["age:"]);
    assert_eq!(labels(&complete(&mut editor, "_ = models.User { ")[..2]), ["name:", "age:"]);
    assert_eq!(labels(&complete(&mut editor, "_ = models.User { name: \"Al\", ")[..1]), ["age:"]);

    // A closure's parameter written without its type shows it.
    let text = with_line(&mut editor, "let doubled = [1, 2].map((n) { n * 2 })");
    let lines = text.lines().count();
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": lines, "character": 0 } });
    let hints = editor.request("textDocument/inlayHint", json!({ "textDocument": { "uri": uri }, "range": range }));
    let labels_of: Vec<&str> = hints.as_array().unwrap().iter().map(|h| h["label"].as_str().unwrap()).collect();
    assert!(labels_of.contains(&": Int") && labels_of.contains(&": Array[Int]"), "{hints}");
    // Unused code is faded.
    with_line(&mut editor, "let unused = 1");
    editor.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } }),
    );
    let (_, diagnostics) = editor.diagnostics.iter().find(|(u, _)| *u == uri).cloned().unwrap();
    let unused = diagnostics.as_array().unwrap().iter().find(|d| d["code"] == "W1002").expect("an unused binding");
    assert_eq!(unused["tags"], json!([1]));

    // What a keyword does.
    let text = with_line(&mut editor, "_ = 1");
    let hover = editor.at("textDocument/hover", &uri, &text, "match models.Role", "match", json!({}));
    assert!(hover["contents"]["value"].as_str().unwrap().contains("Chooses by a value's shape"), "{hover}");
    // While a debugger is stopped at a line, the bindings up to it.
    let stopped = text.lines().position(|l| l.contains("console.println(ada.name)")).unwrap();
    let values = editor.request(
        "textDocument/inlineValue",
        json!({ "textDocument": { "uri": uri }, "range": range,
            "context": { "frameId": 1, "stoppedLocation": { "start": { "line": stopped, "character": 0 }, "end": { "line": stopped, "character": 4 } } } }),
    );
    let names: Vec<&str> = values.as_array().unwrap().iter().map(|v| v["variableName"].as_str().unwrap()).collect();
    assert!(names.contains(&"ada") && names.contains(&"level") && !names.contains(&"text"), "{names:?}");
    // The tokens of a few lines, as of the whole file.
    let some = editor.request(
        "textDocument/semanticTokens/range",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } } }),
    );
    let all = editor.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    let (some, all) = (some["data"].as_array().unwrap().len(), all["data"].as_array().unwrap().len());
    assert!(some > 0 && some < all, "{some} of {all}");

    // A `match` without every case: an arm for the one it misses.
    let text = MAIN.replace("        models.Role.Admin => console.println(\"admin\")\n", "");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": text }] }),
    );
    editor.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } }),
    );
    let (_, diagnostics) = editor.diagnostics.iter().find(|(u, _)| *u == uri).cloned().unwrap();
    let missing = diagnostics.as_array().unwrap().iter().find(|d| d["code"] == "E1204").cloned().expect("E1204");
    let actions = editor.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": missing["range"], "context": { "diagnostics": [missing] } }),
    );
    let titles: Vec<&str> = actions.as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"add an arm for `models.Role.Admin`"), "{titles:?}");
    editor.stop();
}

/// A file changed on disk, as by a checkout, is analyzed again once the
/// editor says so, which the server asks it to watch for.
#[test]
fn files_changed_on_disk() {
    let dir =
        project("lsp-watched", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", "fn main() {}\n")]);
    let mut editor = Editor::start_with_folders(&dir);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    editor.request("workspace/symbol", json!({ "query": "" }));
    let uri = format!("file://{}", dir.join("src/main.to").display());
    std::fs::write(dir.join("src/main.to"), "fn main() {\n    let x: Int = \"text\"\n    _ = x\n}\n").unwrap();
    editor.notify("workspace/didChangeWatchedFiles", json!({ "changes": [{ "uri": uri, "type": 2 }] }));
    editor.request("workspace/symbol", json!({ "query": "" }));
    let (_, diagnostics) = editor.diagnostics.iter().find(|(u, _)| *u == uri).cloned().expect("diagnostics");
    assert_eq!(diagnostics[0]["code"], "E1102", "{diagnostics}");
    editor.stop();
}

const REFACTORED: &str = "\
pub enum ConfigError {
    Invalid
}

pub trait Named {
    fn name(self): String
}

pub type Box = { label: String }

impl Named for Box {
    fn name(self): String {
        self.label
    }
}

fn check(port: Int): Int ! ConfigError {
    if port < 0 {
        fail ConfigError.Invalid
    }
    port
}

fn first(values: Array[Int]): Int? {
    let found = values.first()?
    some(found * 2)
}

fn total(port: Int): Int ! ConfigError {
    let checked = check(port)?
    let base = checked + 1
    let doubled = base * 2 + 10
    doubled * 3 + base
}

fn guarded(values: Array[Int]): Bool {
    values.isEmpty() or values.first() == some(0)
}

fn sum(values: Array[Int]): Int {
    var all = 0
    for value in values {
        all += value
    }
    all
}

fn main() {
    let box = Box { label: \"a\" }
    _ = (Named.name(box), first([1]), total(80), guarded([]), sum([]))
}
";

#[test]
fn explains_counts_and_refactors() {
    let dir = project("lsp-refactor", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", REFACTORED)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    // What a `?` and a `fail` do there.
    let hover = |editor: &mut Editor, line: &str, word: &str| -> String {
        let found = editor.at("textDocument/hover", &uri, REFACTORED, line, word, json!({}));
        found["contents"]["value"].as_str().unwrap_or_default().to_string()
    };
    assert_eq!(
        hover(&mut editor, "let checked = check(port)?", "?"),
        "`?` gives the `Int`, or returns the `ConfigError` from `total`"
    );
    assert_eq!(hover(&mut editor, "values.first()?", "?"), "`?` gives the `Int`, or returns `none` from `first`");
    assert_eq!(hover(&mut editor, "fail ConfigError", "fail"), "`fail` ends `check` with the `ConfigError`");

    // How many places use each declaration, and implement a trait.
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let line_of = |text: &str| REFACTORED.lines().position(|l| l.contains(text)).unwrap() as u64;
    let titles = |line: u64| -> Vec<String> {
        let lenses = lenses.as_array().unwrap().iter().filter(|l| l["range"]["start"]["line"] == line);
        lenses.map(|l| l["command"]["title"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(titles(line_of("pub trait Named")), ["2 references", "1 implementation"], "{lenses}");
    assert_eq!(titles(line_of("fn check(")), ["1 reference"], "{lenses}");
    let check =
        lenses.as_array().unwrap().iter().find(|l| l["range"]["start"]["line"] == line_of("fn check(")).unwrap();
    assert_eq!(check["command"]["command"], "tondo.showReferences");
    assert_eq!(check["command"]["arguments"][2].as_array().unwrap().len(), 1);

    // Refactors of a selection, when asked for.
    let refactors = |editor: &mut Editor, start: (u64, u64), end: (u64, u64)| -> Vec<(String, Value)> {
        let range =
            json!({ "start": { "line": start.0, "character": start.1 }, "end": { "line": end.0, "character": end.1 } });
        let params = json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [], "triggerKind": 1 } });
        let found = editor.request("textDocument/codeAction", params);
        let actions = found.as_array().unwrap().iter().filter(|a| a["kind"] == "refactor.extract");
        actions.map(|a| (a["title"].as_str().unwrap().to_string(), a["edit"]["changes"][&uri].clone())).collect()
    };
    let doubled = line_of("let doubled");
    let found = refactors(&mut editor, (doubled, 18), (doubled, 26));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "Extract into a variable");
    assert_eq!(found[0].1[0]["newText"], "    let value = base * 2\n");
    assert_eq!(found[0].1[1]["newText"], "value");
    // Lines into a function, with what they read as parameters, what is
    // read after them as its result, and the error it propagates.
    let found = refactors(&mut editor, (line_of("let checked"), 4), (doubled, 31));
    assert_eq!(found.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["Extract into a function"], "{found:?}");
    assert_eq!(found[0].1[0]["newText"], "    let (base, doubled) = extracted(port)?");
    assert_eq!(
        found[0].1[1]["newText"],
        "\n\nfn extracted(port: Int): (Int, Int) ! ConfigError {\n    let checked = check(port)?\n    let base = checked + 1\n    let doubled = base * 2 + 10\n    (base, doubled)\n}"
    );
    // Not what may not run where it would go; a `var` of its function is
    // lent with `mut`.
    let guarded = line_of("values.isEmpty() or");
    assert!(refactors(&mut editor, (guarded, 24), (guarded, 49)).is_empty());
    let found = refactors(&mut editor, (line_of("for value in values"), 4), (line_of("all += value") + 1, 5));
    assert_eq!(found[0].1[0]["newText"], "    extracted(values, mut all)", "{found:?}");
    // The light bulb does not ask for them.
    let params = json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": doubled, "character": 18 }, "end": { "line": doubled, "character": 26 } }, "context": { "diagnostics": [], "triggerKind": 2 } });
    let automatic = editor.request("textDocument/codeAction", params);
    assert!(automatic.as_array().unwrap().iter().all(|a| a["kind"] != "refactor.extract"), "{automatic}");

    // A method an `impl` lacks and fields a literal lacks are written for
    // the program to fill.
    let broken = "pub type Counter = { n: Int, label: String }\n\nimpl Iterator[Int] for Counter {}\n\nlet counter = Counter {}\n_ = counter\n";
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": broken }] }),
    );
    let repairs = |editor: &mut Editor, line: u64, character: u64| -> Vec<(String, String)> {
        let at = json!({ "line": line, "character": character });
        let params = json!({ "textDocument": { "uri": uri }, "range": { "start": at, "end": at }, "context": { "diagnostics": [] } });
        let found = editor.request("textDocument/codeAction", params);
        let fixes = found.as_array().unwrap().iter().filter(|a| a["kind"] == "quickfix");
        fixes
            .map(|a| {
                (
                    a["title"].as_str().unwrap().to_string(),
                    a["edit"]["changes"][&uri][0]["newText"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    assert_eq!(
        repairs(&mut editor, 2, 1),
        [(
            "write `next`".to_string(),
            "\n    fn next(mut self): Int? {\n        panic(\"`next` is not written yet\")\n    }\n".to_string()
        )]
    );
    assert_eq!(
        repairs(&mut editor, 4, 16),
        [("add the fields `n: Int` and `label: String`".to_string(), " n: 0, label: \"\" ".to_string())]
    );
    editor.stop();
}

const SHOWN: &str = "import std.console
import std.image

pub enum Problem {
    Empty
}

fn parse(text: String, base: Int): Int ! Problem {
    if text.isEmpty() {
        fail Problem.Empty
    }
    let checked = text.length()
    if checked > 10 {
        return base
    }
    checked + base
}

fn total(values: Array[Int]): Int {
    var sum = 0
    for value in values {
        sum += value
    }
    let doubled = values
        .map((n) { n * 2 })
        .filter((n) { n > 2 })
    let count = doubled.length()
    sum + count
}

fn main() {
    let orange = image.Color.hex(\"#ff8800\")
    let blue = image.Color.rgb(0, 0, 255)
    let size: Int = total([1, 2])
    console.println(\"{orange.red} {blue.blue} {size} {parse(\"a\", 1)}\")
}
";

#[test]
fn writes_tests_completes_and_shows_more() {
    let dir = project("lsp-more", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", SHOWN)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let line_of = |text: &str, part: &str| text.lines().position(|l| l.contains(part)).unwrap() as u64;
    let column = |text: &str, part: &str, word: &str| {
        let line = text.lines().find(|l| l.contains(part)).unwrap();
        (line.find(part).unwrap() + part.find(word).unwrap()) as u64
    };
    let at = |text: &str, part: &str, word: &str| json!({ "line": line_of(text, part), "character": column(text, part, word) });
    let slice = |text: &str, range: &Value| -> String {
        let line = text.lines().nth(range["start"]["line"].as_u64().unwrap() as usize).unwrap();
        let (from, to) = (
            range["start"]["character"].as_u64().unwrap() as usize,
            range["end"]["character"].as_u64().unwrap() as usize,
        );
        line[from..to].to_string()
    };

    // A binding that can change is `mutable`, the fourth modifier.
    let tokens = editor.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    let data: Vec<u64> = tokens["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let (mut line, mut start, mut changing) = (0u64, 0u64, Vec::new());
    for token in data.chunks(5) {
        line += token[0];
        start = if token[0] == 0 { start + token[1] } else { token[1] };
        if token[4] & 8 != 0 {
            let text = SHOWN.lines().nth(line as usize).unwrap();
            changing.push(text[start as usize..(start + token[2]) as usize].to_string());
        }
    }
    assert_eq!(changing, ["sum", "sum", "sum"], "{tokens}");

    // On `fn`, where the function leaves.
    let found = editor.request(
        "textDocument/documentHighlight",
        json!({ "textDocument": { "uri": uri }, "position": at(SHOWN, "fn parse", "fn") }),
    );
    let exits: Vec<String> = found.as_array().unwrap().iter().map(|h| slice(SHOWN, &h["range"])).collect();
    assert_eq!(exits, ["fail", "return"], "{found}");

    // The type of each line of a chain.
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 99, "character": 0 } });
    let hints = editor.request("textDocument/inlayHint", json!({ "textDocument": { "uri": uri }, "range": range }));
    let chained: Vec<(u64, &str)> = hints
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["paddingLeft"] == true)
        .map(|h| (h["position"]["line"].as_u64().unwrap(), h["label"].as_str().unwrap()))
        .collect();
    assert_eq!(chained, [(line_of(SHOWN, "let doubled"), ": Array[Int]"), (line_of(SHOWN, ".map("), ": Array[Int]")]);

    // A test of a function, in the module's tests.
    let here = at(SHOWN, "fn parse", "parse");
    let params = json!({ "textDocument": { "uri": uri }, "range": { "start": here, "end": here }, "context": { "diagnostics": [] } });
    let found = editor.request("textDocument/codeAction", params);
    let test = found.as_array().unwrap().iter().find(|a| a["title"] == "Write a test of `parse`").expect("a test");
    let changes = &test["edit"]["documentChanges"];
    let tests = format!("file://{}", dir.join("src/main_test.to").display());
    assert_eq!(changes[0]["kind"], "create");
    assert_eq!(changes[0]["uri"], tests.as_str());
    assert_eq!(
        changes[1]["edits"][0]["newText"],
        "import std.testing\n\ntest parseWorks {\n    let result = parse(\"\", 0)?\n    _ = result\n    testing.failNow(\"write what `parse` should do, with `assert`\")\n}\n"
    );

    // A binding read once in the next statement, written there instead.
    let here = at(SHOWN, "let count", "count");
    let params = json!({ "textDocument": { "uri": uri }, "range": { "start": here, "end": here }, "context": { "diagnostics": [], "triggerKind": 1 } });
    let found = editor.request("textDocument/codeAction", params);
    let inline = found.as_array().unwrap().iter().find(|a| a["kind"] == "refactor.inline").expect("an inline");
    assert_eq!(inline["title"], "Inline `count`");
    let edits = &inline["edit"]["changes"][&uri];
    assert_eq!(edits[0]["newText"], "");
    assert_eq!(edits[1]["newText"], "doubled.length()");
    // Not a `var`.
    let here = at(SHOWN, "var sum", "sum");
    let params = json!({ "textDocument": { "uri": uri }, "range": { "start": here, "end": here }, "context": { "diagnostics": [], "triggerKind": 1 } });
    let found = editor.request("textDocument/codeAction", params);
    assert!(found.as_array().unwrap().iter().all(|a| a["kind"] != "refactor.inline"), "{found}");

    // The colours of `Color.hex` and `Color.rgb`, and how a picked one is written.
    let colors = editor.request("textDocument/documentColor", json!({ "textDocument": { "uri": uri } }));
    let colors = colors.as_array().unwrap();
    assert_eq!(colors.iter().map(|c| slice(SHOWN, &c["range"])).collect::<Vec<_>>(), ["\"#ff8800\"", "0, 0, 255"]);
    assert_eq!(colors[0]["color"]["red"], 1.0);
    let picked = json!({ "red": 0.5, "green": 0.0, "blue": 1.0, "alpha": 1.0 });
    let written = |editor: &mut Editor, range: &Value| {
        let params = json!({ "textDocument": { "uri": uri }, "color": picked, "range": range });
        editor.request("textDocument/colorPresentation", params)[0]["label"].as_str().unwrap().to_string()
    };
    assert_eq!(written(&mut editor, &colors[0]["range"]), "\"#8000ff\"");
    assert_eq!(written(&mut editor, &colors[1]["range"]), "128, 0, 255");

    // After a value's dot, what turns it into a construction around it.
    let text = SHOWN.replace("    let count = doubled.length()\n", "    let count = doubled.length()\n    count.ma\n");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let mut position = at(&text, "count.ma", "ma");
    position["character"] = json!(position["character"].as_u64().unwrap() + 2);
    let found =
        editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
    let postfix: Vec<(&str, &str)> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == 15)
        .map(|i| (i["label"].as_str().unwrap(), i["textEdit"]["newText"].as_str().unwrap()))
        .collect();
    assert_eq!(
        postfix,
        [
            ("match", "match count {\n\t$0\n}"),
            ("let", "let ${1:count} = count"),
            ("some", "some(count)"),
            ("ok", "ok(count)")
        ]
    );

    // What the type written before `=` takes comes first, but not the binding itself.
    let text = SHOWN
        .replace("    let size: Int = total([1, 2])\n", "    let size: Int = total([1, 2])\n    let other: Int = \n");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": text }] }),
    );
    let mut position = at(&text, "let other: Int = ", "= ");
    position["character"] = json!(position["character"].as_u64().unwrap() + 2);
    let found =
        editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
    let first: Vec<&str> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["preselect"] == true)
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert_eq!(first, ["size", "total"], "{found}");
    editor.stop();
}

#[test]
fn renames_where_a_binding_is_assigned() {
    let dir = project(
        "lsp-assigned",
        &[("main.to", "fn main() {\n    var sum = 0\n    sum += 1\n    sum = sum * 2\n    _ = sum\n}\n")],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    let params =
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 8 }, "newName": "total" });
    let found = editor.request("textDocument/rename", params);
    assert_eq!(found["changes"][&uri].as_array().unwrap().len(), 5, "{found}");
    editor.stop();
}

const SHAPES: &str = "pub enum Shape {
    Dot
    Circle(Float)
    Rect { width: Float, height: Float }
}

fn area(shape: Shape): Float {
    0.0
}
";

const COUNTED: &str = "import app.shapes
import std.console
import std.embed

pub type Counter = { count: Int, step: Int }

fn Counter.advance(mut self, times: Int) {
    var total = 0
    for _ in 0..times {
        total += self.step
        self.count += self.step
    }
    console.println(\"{total}\")
}

fn describe(shape: shapes.Shape): String {
    match shape {
        shapes.Shape.Dot => \"dot\"
    }
}

fn main() {
    var counter = Counter { count: 0, step: 2 }
    counter.advance(3)
    console.println(\"{describe(shapes.Shape.Dot)} {shapes.area(shapes.Shape.Dot)} {embed.text(\"../assets/greeting.txt\")}\")
}
";

#[test]
fn fills_arms_publishes_extracts_and_links() {
    let dir = project(
        "lsp-fourth",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/shapes/shapes.to", SHAPES),
            ("src/main.to", COUNTED),
            ("assets/greeting.txt", "hello\n"),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let shapes = format!("file://{}", dir.join("src/shapes/shapes.to").display());
    let line_of = |part: &str| COUNTED.lines().position(|l| l.contains(part)).unwrap() as u64;
    let actions = |editor: &mut Editor, start: (u64, u64), end: (u64, u64)| -> Vec<Value> {
        let range =
            json!({ "start": { "line": start.0, "character": start.1 }, "end": { "line": end.0, "character": end.1 } });
        let params = json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [], "triggerKind": 1 } });
        editor.request("textDocument/codeAction", params).as_array().unwrap().clone()
    };
    let titled = |found: &[Value], title: &str| found.iter().find(|a| a["title"] == title).cloned();

    // A function another module may not use: `pub`, in its own file.
    let area = line_of("shapes.area(");
    let column = COUNTED.lines().nth(area as usize).unwrap().find("area(").unwrap() as u64;
    let found = actions(&mut editor, (area, column), (area, column));
    let public = titled(&found, "declare `area` `pub`").expect("a repair");
    assert_eq!(public["edit"]["changes"][&shapes][0]["newText"], "pub ");
    assert_eq!(public["edit"]["changes"][&shapes][0]["range"]["start"]["line"], 6);
    let fixed = SHAPES.replace("fn area", "pub fn area");
    std::fs::write(dir.join("src/shapes/shapes.to"), &fixed).unwrap();
    editor.notify("workspace/didChangeWatchedFiles", json!({ "changes": [{ "uri": shapes, "type": 2 }] }));

    // A `match` gets an arm for each case it misses.
    let matched = line_of("match shape");
    let found = actions(&mut editor, (matched, 4), (matched, 4));
    let every = titled(&found, "add an arm for each of the 2 cases missing").expect("every arm");
    assert_eq!(
        every["edit"]["changes"][&uri][0]["newText"],
        "        shapes.Shape.Circle(_) => panic(\"`shapes.Shape.Circle(_)` is not handled yet\")\n        shapes.Shape.Rect { .. } => panic(\"`shapes.Shape.Rect { .. }` is not handled yet\")\n"
    );

    // Lines that use `self` and a `var` of the method become a method that
    // borrows the `var`.
    let found = actions(&mut editor, (line_of("for _ in 0..times"), 4), (line_of("self.count += self.step") + 1, 5));
    let extract = titled(&found, "Extract into a function").expect("an extraction");
    let edits = &extract["edit"]["changes"][&uri];
    assert_eq!(edits[0]["newText"], "    self.extracted(times, mut total)");
    assert_eq!(
        edits[1]["newText"],
        "\n\nfn Counter.extracted(mut self, times: Int, total: mut Int) {\n    for _ in 0..times {\n        total += self.step\n        self.count += self.step\n    }\n}"
    );

    // Links: an import to its module, an embedded path to its file.
    let links = editor.request("textDocument/documentLink", json!({ "textDocument": { "uri": uri } }));
    let targets: Vec<&str> = links.as_array().unwrap().iter().map(|l| l["target"].as_str().unwrap()).collect();
    assert!(targets.contains(&shapes.as_str()), "{links}");
    let greeting = format!("file://{}", dir.join("assets/greeting.txt").display());
    assert!(targets.contains(&greeting.as_str()), "{links}");
    editor.stop();
}

#[test]
fn completes_and_finds_what_is_not_imported() {
    let dir = project(
        "lsp-unimported",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/handlers/handlers.to", "pub fn handleRequest(path: String): String {\n    path\n}\n"),
            ("src/main.to", "import std.console\n\nfn main() {\n    console.println(\"x\")\n}\n"),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    // A name of a module the file does not import, with the import.
    let complete = |editor: &mut Editor, typed: &str| -> Vec<(String, String, String)> {
        let text = format!("import std.console\n\nfn main() {{\n    console.println(\"x\")\n    let v = {typed}\n}}\n");
        editor.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
        let position = json!({ "line": 4, "character": 12 + typed.len() });
        let found =
            editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": position }));
        let items = found["items"].as_array().unwrap().iter().filter(|i| i["labelDetails"]["description"].is_string());
        items
            .map(|i| {
                let import = i["additionalTextEdits"][0]["newText"].as_str().unwrap_or_default().to_string();
                (i["label"].as_str().unwrap().to_string(), i["insertText"].as_str().unwrap().to_string(), import)
            })
            .collect()
    };
    let uuid = complete(&mut editor, "Uui");
    assert_eq!(uuid[0], ("Uuid".to_string(), "uuid.Uuid".to_string(), "import std.uuid\n".to_string()), "{uuid:?}");
    let own = complete(&mut editor, "handleR");
    assert_eq!(
        own[0],
        ("handleRequest".into(), "handlers.handleRequest".into(), "import app.handlers\n".into()),
        "{own:?}"
    );
    // Symbols by the letters of their words.
    let found = editor.request("workspace/symbol", json!({ "query": "hRq" }));
    assert_eq!(found[0]["name"], "handleRequest", "{found}");
    editor.stop();
}

#[test]
fn points_to_related_places_and_renames_unused() {
    let text = "fn helper(): Int {\n    1\n}\n\nfn helper(): Int {\n    2\n}\n\nfn main() {\n    let unused = 1\n    _ = helper()\n}\n";
    let dir = project("lsp-related", &[("main.to", text)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("main.to"));
    editor.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } }),
    );
    // A second declaration points to the first.
    let (_, diagnostics) = editor.diagnostics.iter().find(|(u, _)| *u == uri).cloned().unwrap();
    let repeated = diagnostics.as_array().unwrap().iter().find(|d| d["code"] == "E1002").expect("a duplicate");
    let related = &repeated["relatedInformation"][0];
    assert_eq!(related["message"], "`helper` is first declared here", "{repeated}");
    assert_eq!(related["location"]["range"]["start"]["line"], 0);
    // A binding nothing uses is renamed to say so, as a decision.
    let text = "fn main() {\n    let unused = 1\n}\n";
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let at = json!({ "line": 1, "character": 8 });
    let params = json!({ "textDocument": { "uri": uri }, "range": { "start": at, "end": at }, "context": { "diagnostics": [] } });
    let found = editor.request("textDocument/codeAction", params);
    let rename = found.as_array().unwrap().iter().find(|a| a["title"] == "rename it `_unused`").expect("a rename");
    assert_eq!(rename["isPreferred"], false);
    assert_eq!(rename["edit"]["changes"][&uri][0]["newText"], "_unused");
    editor.stop();
}

#[test]
fn runs_tests_from_lenses_and_formats_closed_blocks() {
    let dir = project(
        "lsp-run-tests",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", "fn main() {}\n\npub fn double(n: Int): Int {\n    n * 2\n}\n"),
            (
                "src/main_test.to",
                "test doubles {\n    assert(double(2) == 4)\n}\n\ntest fails {\n    assert(double(2) == 5)\n}\n",
            ),
        ],
    );
    // An editor that does not run tests itself gets a lens over each.
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main_test.to"));
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let runs: Vec<Value> = lenses
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["command"]["command"] == "tondo.runTest" && l["command"]["title"] != "Every order")
        .map(|l| l["command"].clone())
        .collect();
    let ids: Vec<&str> = runs.iter().map(|c| c["arguments"][1].as_str().unwrap()).collect();
    assert_eq!(ids, ["app::unit::app::doubles", "app::unit::app::fails"], "{lenses}");
    // Running one says how it went, and why it failed.
    let said = |editor: &mut Editor, command: &Value| -> String {
        editor.request(
            "workspace/executeCommand",
            json!({ "command": "tondo.runTest", "arguments": command["arguments"] }),
        );
        loop {
            let message = editor.receive();
            if message["method"] == "window/showMessage" {
                return message["params"]["message"].as_str().unwrap().to_string();
            }
        }
    };
    assert_eq!(said(&mut editor, &runs[0]), "`app::unit::app::doubles` passed");
    assert_eq!(
        said(&mut editor, &runs[1]),
        "`app::unit::app::fails` failed: panic[P0007]: assertion-failed: double(2) == 5"
    );
    editor.stop();

    // A block closed with `}` takes the canonical layout.
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let text = "fn main() {}\n\npub fn double(n: Int): Int {\n    if n>1 {\n  return n*2\n}\n    n * 2\n}\n";
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let params = json!({ "textDocument": { "uri": uri }, "position": { "line": 5, "character": 1 }, "ch": "}",
        "options": { "tabSize": 4, "insertSpaces": true } });
    let edits = editor.request("textDocument/onTypeFormatting", params);
    assert_eq!(edits[0]["newText"], "    if n > 1 {\n        return n * 2\n    }\n", "{edits}");
    editor.stop();
}

/// The edits of an answer, each file's as it ends up.
fn applied(answer: &Value, root: &Path) -> std::collections::HashMap<String, String> {
    let changes = answer["changes"].as_object().unwrap_or_else(|| panic!("{answer}"));
    let mut files = std::collections::HashMap::new();
    for (uri, edits) in changes {
        let path = uri.strip_prefix("file://").unwrap();
        let mut text = std::fs::read_to_string(path).unwrap();
        let offset = |text: &str, at: &Value| {
            let line = at["line"].as_u64().unwrap() as usize;
            let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
            let rest = &text[start..];
            let chars = at["character"].as_u64().unwrap() as usize;
            start + rest.char_indices().nth(chars).map_or(rest.len(), |(at, _)| at)
        };
        let mut edits: Vec<&Value> = edits.as_array().unwrap().iter().collect();
        edits.sort_by_key(|edit| std::cmp::Reverse(offset(&text, &edit["range"]["start"])));
        for edit in edits {
            let (from, to) = (offset(&text, &edit["range"]["start"]), offset(&text, &edit["range"]["end"]));
            text.replace_range(from..to, edit["newText"].as_str().unwrap());
        }
        let name = Path::new(path).strip_prefix(root).unwrap().display().to_string();
        files.insert(name, text);
    }
    files
}

#[test]
fn changes_signatures_moves_declarations_and_inlines_calls() {
    let tondo_toml = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
    let main = "import app.tools\nimport std.console\n\nfn twice(value: Int): Int {\n    value * 2\n}\n\n\
        fn label(name: String, times: Int): String {\n    \"{name} {times}\"\n}\n\nfn main() {\n    \
        console.println(label(\"a, b\", twice(2)))\n    console.println(\"{tools.size(\"x\", 3)}\")\n}\n";
    let tools = "pub fn size(text: String, extra: Int): Int {\n    text.length() + extra\n}\n\npub fn idle(): Int {\n    0\n}\n";
    let other = "pub fn nothing(): Int {\n    1\n}\n";
    let dir = project(
        "lsp-refactors",
        &[
            ("tondo.toml", tondo_toml),
            ("src/main.to", main),
            ("src/tools/tools.to", tools),
            ("src/other/other.to", other),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let tools_uri = editor.open(&dir.join("src/tools/tools.to"));
    let position = |text: &str, word: &str| {
        let at = text.find(word).unwrap();
        let line = text[..at].matches('\n').count();
        let character = at - text[..at].rfind('\n').map_or(0, |i| i + 1);
        json!({ "line": line, "character": character })
    };
    let actions = |editor: &mut Editor, uri: &str, at: Value| {
        let range = json!({ "start": at, "end": at });
        let params = json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [] } });
        editor.request("textDocument/codeAction", params).as_array().unwrap().clone()
    };

    // The editor is offered what it then asks for.
    let found = actions(&mut editor, &uri, position(main, "label(name"));
    let change = found.iter().find(|a| a["title"] == "Change the signature of `label`").unwrap();
    assert_eq!(change["command"]["arguments"][2], "name: String, times: Int");

    // Parameters reordered, one added: every call follows, also across a
    // comma inside a text.
    let at = position(main, "label(name");
    let wanted = "times: Int, name: String, tail: String = \"!\"";
    let params = json!({ "command": "tondo.signature", "arguments": [uri, at, wanted] });
    let answer = editor.request("workspace/executeCommand", params);
    let edited = applied(&answer["edit"], &dir);
    let text = &edited["src/main.to"];
    assert!(text.contains("fn label(times: Int, name: String, tail: String): String"), "{text}");
    assert!(text.contains("label(twice(2), \"a, b\", \"!\")"), "{text}");

    // What would not compile is refused, saying why.
    let params =
        json!({ "command": "tondo.signature", "arguments": [uri, position(main, "label(name"), "name: String"] });
    let answer = editor.request("workspace/executeCommand", params);
    assert!(answer["error"].as_str().unwrap().contains("would not check"), "{answer}");

    // A call of a function of one expression.
    let found = actions(&mut editor, &uri, position(main, "twice(2)"));
    let inline =
        found.iter().find(|a| a["title"] == "Inline this call of `twice`").unwrap_or_else(|| panic!("{found:?}"));
    let text = &applied(&inline["edit"], &dir)["src/main.to"];
    assert!(text.contains("label(\"a, b\", (2 * 2))"), "{text}");

    // To another module: the declaration, and all that names it.
    let params = json!({ "command": "tondo.modules", "arguments": [tools_uri, position(tools, "size")] });
    let modules = editor.request("workspace/executeCommand", params);
    assert!(modules.as_array().unwrap().iter().any(|m| m == "app.other"), "{modules}");
    let params = json!({ "command": "tondo.moveTo", "arguments": [tools_uri, position(tools, "size"), "app.other"] });
    let answer = editor.request("workspace/executeCommand", params);
    let edited = applied(&answer["edit"], &dir);
    assert!(edited["src/other/other.to"].contains("pub fn size(text: String, extra: Int): Int {"), "{edited:?}");
    assert!(!edited["src/tools/tools.to"].contains("fn size"), "{edited:?}");
    assert!(edited["src/main.to"].contains("import app.other\n"), "{edited:?}");
    assert!(edited["src/main.to"].contains("{other.size(\"x\", 3)}"), "{edited:?}");

    // `pub` for nothing, in a program: a hint, and its repair.
    let (path, hints) = editor
        .diagnostics
        .iter()
        .rev()
        .find(|(path, _)| path.ends_with("tools.to"))
        .cloned()
        .unwrap_or_else(|| panic!("no diagnostics of tools.to"));
    let hint =
        hints.as_array().unwrap().iter().find(|d| d["code"] == "H1001").unwrap_or_else(|| panic!("{path}: {hints}"));
    assert!(hint["message"].as_str().unwrap().contains("`idle`"), "{hint}");
    assert_eq!(hint["severity"], 4);
    let found = actions(&mut editor, &tools_uri, position(tools, "pub fn idle"));
    assert!(found.iter().any(|a| a["title"] == "remove `pub`"), "{found:?}");
    editor.stop();
}

#[test]
fn a_hover_says_what_an_open_dump_held() {
    let source = "fn work(limit: Int): Int {\n    let doubled = limit * 2\n    if doubled > 0 {\n        panic(\"look\")\n    }\n    doubled\n}\n\nfn main() {\n    _ = work(21)\n}\n";
    let dir = project("lsp-dump", &[("late.to", source), ("dumps/.keep", "")]);
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built = std::process::Command::new(tondo)
        .args(["build", "late.to", "--debug", "--output", "late"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    std::process::Command::new(dir.join("late")).env("TONDO_DUMP", dir.join("dumps")).output().unwrap();
    let dump = std::fs::read_dir(dir.join("dumps"))
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .find(|p| p.to_string_lossy().contains(".dump.json"))
        .unwrap();
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("late.to"));
    editor.request("workspace/executeCommand", json!({ "command": "tondo.useDump", "arguments": [dump] }));
    let position = json!({ "line": 1, "character": 8 });
    let hover = editor.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": position }));
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("In the dump:") && text.contains("`42`"), "{text}");
    // Without it, as ever.
    editor.request("workspace/executeCommand", json!({ "command": "tondo.useDump", "arguments": [null] }));
    let hover = editor.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": position }));
    assert!(!hover["contents"]["value"].as_str().unwrap().contains("In the dump:"), "{hover}");
    editor.stop();
}

#[test]
fn says_what_can_fail_deletes_what_is_unused_and_groups_parameters() {
    let tondo_toml = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
    let main = "import app.shapes\nimport std.console\n\nfn inner(value: Int): Int {\n    if value < 0 {\n        panic(\"negative\")\n    }\n    value\n}\n\n\
        fn outer(value: Int): Int {\n    // Not a panic(\"here\").\n    inner(value) + 1\n}\n\n/// Nobody calls this.\nfn unused(): Int {\n    1\n}\n\n\
        fn main() {\n    console.println(\"{outer(1)} {shapes.area(2, 3, \"m\")}\")\n}\n";
    let shapes = "pub fn area(width: Int, height: Int, unit: String): String {\n    \"{width * height} {unit}\"\n}\n";
    let dir = project(
        "lsp-fail-delete-group",
        &[
            ("tondo.toml", tondo_toml),
            ("src/main.to", main),
            ("src/main_test.to", "test viaOuter {\n    assert(outer(1) > 0)\n}\n"),
            ("src/shapes/shapes.to", shapes),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let shapes_uri = editor.open(&dir.join("src/shapes/shapes.to"));
    let position = |text: &str, word: &str| {
        let at = text.find(word).unwrap();
        let line = text[..at].matches('\n').count();
        json!({ "line": line, "character": at - text[..at].rfind('\n').map_or(0, |i| i + 1) })
    };
    // What a call can end in, through what it calls.
    let at = position(main, "outer(value");
    let hover = editor.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": at }));
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("Can fail:") && text.contains("panics in `inner` at src/main.to:6"), "{text}");
    assert_eq!(text.matches("panics").count(), 1, "{text}");

    // What reaches a function, through what calls it.
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let titled = |line: u64| {
        let lenses = lenses.as_array().unwrap().iter();
        let found = lenses.filter(|l| l["range"]["start"]["line"] == line);
        found.filter_map(|l| l["command"]["title"].as_str()).any(|title| title == "1 test reaches this")
    };
    let line = |word: &str| main[..main.find(word).unwrap()].matches('\n').count() as u64;
    assert!(titled(line("inner(value:")) && titled(line("outer(value:")), "{lenses}");
    assert!(!titled(line("unused()")), "{lenses}");

    // What nothing uses goes, with its comment; what something uses, not.
    let actions = |editor: &mut Editor, at: Value| {
        let range = json!({ "start": at, "end": at });
        let params = json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [] } });
        editor.request("textDocument/codeAction", params).as_array().unwrap().clone()
    };
    let found = actions(&mut editor, position(main, "unused(): Int"));
    let delete = found
        .iter()
        .find(|a| a["title"] == "Delete `unused`, which nothing uses")
        .unwrap_or_else(|| panic!("{found:?}"));
    let text = &applied(&delete["edit"], &dir)["src/main.to"];
    assert!(!text.contains("unused") && !text.contains("Nobody calls"), "{text}");
    let found = actions(&mut editor, position(main, "inner(value: Int)"));
    assert!(!found.iter().any(|a| a["title"].as_str().unwrap().starts_with("Delete")), "{found:?}");

    // Parameters into a type, in another file's calls too.
    let at = position(shapes, "area(width");
    let params = json!({ "command": "tondo.group", "arguments": [shapes_uri, at, "Size: width, height"] });
    let answer = editor.request("workspace/executeCommand", params);
    let edited = applied(&answer["edit"], &dir);
    let text = &edited["src/shapes/shapes.to"];
    assert!(text.contains("pub type Size = {\n    width: Int\n    height: Int\n}\n"), "{text}");
    assert!(text.contains("pub fn area(size: Size, unit: String): String"), "{text}");
    assert!(text.contains("{size.width * size.height}"), "{text}");
    let text = &edited["src/main.to"];
    assert!(text.contains("shapes.area(shapes.Size { width: 2, height: 3 }, \"m\")"), "{text}");
    editor.stop();
}

#[test]
fn extracts_a_trait_links_names_and_answers_for_diagnostics() {
    let main = "pub type Square = { side: Int }\n\n/// Its surface.\npub fn Square.area(self): Int {\n    \
        self.side * self.side\n}\n\npub fn Square.name(self): String {\n    \"square\"\n}\n\n\
        pub fn Square.twice(self): Int {\n    self.area() * 2\n}\n\nfn main() {\n    let shape = Square { side: 2 }\n    \
        _ = shape.twice() + shape.name().length()\n}\n";
    let dir = project(
        "lsp-trait",
        &[("tondo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n"), ("src/main.to", main)],
    );
    let mut editor = Editor::start_with(json!({ "pullDiagnostics": true }));
    let uri = editor.open(&dir.join("src/main.to"));
    let position = |word: &str| {
        let at = main.find(word).unwrap();
        json!({ "line": main[..at].matches('\n').count(), "character": at - main[..at].rfind('\n').map_or(0, |i| i + 1) })
    };
    let at = position("area(self)");
    let params = json!({ "command": "tondo.trait", "arguments": [uri, at, "Shape: area, name"] });
    let answer = editor.request("workspace/executeCommand", params);
    assert!(answer["error"].is_null(), "{answer}");
    let text = &applied(&answer["edit"], &dir)["src/main.to"];
    assert!(text.contains("pub trait Shape {\n    fn area(self): Int\n    fn name(self): String\n}"), "{text}");
    assert!(text.contains("impl Shape for Square {\n    fn area(self): Int {\n        self.area()\n    }"), "{text}");
    assert!(text.contains("pub fn Square.area(self): Int") && text.contains("self.area() * 2"), "{text}");

    // A local's name, all of it at once; not one another file may name.
    let params = json!({ "textDocument": { "uri": uri }, "position": position("shape =") });
    let linked = editor.request("textDocument/linkedEditingRange", params);
    assert_eq!(linked["ranges"].as_array().unwrap().len(), 3, "{linked}");
    let params = json!({ "textDocument": { "uri": uri }, "position": position("Square = {") });
    assert_eq!(editor.request("textDocument/linkedEditingRange", params), Value::Null);

    // Asked, it says what it would have told.
    let wrong = format!("{main}\nfn broken(): Int {{\n    \"no\"\n}}\n");
    editor.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": wrong }] }),
    );
    let report = editor.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(report["kind"], "full");
    assert!(report["items"].as_array().unwrap().iter().any(|d| d["severity"] == 1), "{report}");
    editor.stop();
}

#[test]
fn moves_between_functions_and_methods_and_inlines_aliases() {
    let main = "import app.shapes\n\npub type Square = { side: Int }\n\nalias Side = Int\n\n\
        pub fn area(square: Square, by: Side): Int {\n    square.side * square.side * by\n}\n\n\
        pub fn Square.twice(self, more: Int): Int {\n    self.side * 2 + more\n}\n\n\
        fn main() {\n    let one = Square { side: 2 }\n    _ = area(one, 3) + one.twice(1) + shapes.same(one)\n}\n";
    let shapes = "import app\n\npub fn same(square: app.Square): Int {\n    app.area(square, 1) + square.twice(0)\n}\n";
    let dir = project(
        "lsp-to-method",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n"),
            ("src/main.to", main),
            ("src/shapes/shapes.to", shapes),
        ],
    );
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let position = |word: &str| {
        let at = main.find(word).unwrap();
        json!({ "line": main[..at].matches('\n').count(), "character": at - main[..at].rfind('\n').map_or(0, |i| i + 1) })
    };
    let run = |editor: &mut Editor, command: &str, word: &str| {
        let params = json!({ "command": command, "arguments": [uri, position(word)] });
        let answer = editor.request("workspace/executeCommand", params);
        assert!(answer["error"].is_null(), "{answer}");
        applied(&answer["edit"], &dir)
    };
    let edited = run(&mut editor, "tondo.toMethod", "area(square");
    let text = &edited["src/main.to"];
    assert!(text.contains("pub fn Square.area(self, by: Side): Int {\n    self.side * self.side * by"), "{text}");
    assert!(text.contains("one.area(3)"), "{text}");
    assert!(edited["src/shapes/shapes.to"].contains("square.area(1)"), "{}", edited["src/shapes/shapes.to"]);

    let edited = run(&mut editor, "tondo.toFunction", "twice(self");
    let text = &edited["src/main.to"];
    assert!(text.contains("pub fn twice(square: Square, more: Int): Int {\n    square.side * 2 + more"), "{text}");
    assert!(text.contains("twice(one, 1)"), "{text}");
    assert!(edited["src/shapes/shapes.to"].contains("app.twice(square, 0)"), "{}", edited["src/shapes/shapes.to"]);

    let at = position("Side = Int");
    let range = json!({ "start": at, "end": at });
    let params = json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [] } });
    let found = editor.request("textDocument/codeAction", params).to_string();
    assert!(found.contains("Inline the alias `Side`"), "{found}");
    let edited = run(&mut editor, "tondo.inlineAlias", "Side = Int");
    let text = &edited["src/main.to"];
    assert!(text.contains("by: Int") && !text.contains("alias"), "{text}");
    editor.stop();
}

#[test]
fn writes_constants_and_branches_and_marks_what_is_deprecated() {
    let main = "/// Old.\n@deprecated(\"use `fresh`\")\npub fn stale(n: Int): Int {\n    n\n}\n\npub fn fresh(n: Int): Int {\n    \
        if n > 3 {\n        n * 60\n    } else {\n        stale(n)\n    }\n}\n\nfn main() {\n    _ = fresh(1)\n}\n";
    let dir = project(
        "lsp-branches",
        &[("tondo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n"), ("src/main.to", main)],
    );
    let mut editor = Editor::start_with(json!({ "pullDiagnostics": true }));
    let uri = editor.open(&dir.join("src/main.to"));
    let position = |word: &str| {
        let at = main.find(word).unwrap();
        json!({ "line": main[..at].matches('\n').count(), "character": at - main[..at].rfind('\n').map_or(0, |i| i + 1) })
    };
    // What goes away shows struck through, saying what to use.
    let report = editor.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let items = report["items"].as_array().unwrap();
    let warned = items.iter().find(|d| d["code"] == "W1015").unwrap_or_else(|| panic!("{report}"));
    assert_eq!(warned["tags"], json!([2]), "{warned}");
    assert!(warned["message"].as_str().unwrap().contains("use `fresh`"), "{warned}");

    let params = json!({ "command": "tondo.ifMatch", "arguments": [uri, position("if n > 3")] });
    let answer = editor.request("workspace/executeCommand", params);
    assert!(answer["error"].is_null(), "{answer}");
    let text = applied(&answer["edit"], &dir)["src/main.to"].clone();
    assert!(text.contains("match n > 3 {\n        true => {\n            n * 60\n        }\n        false => {\n            stale(n)\n        }\n    }"), "{text}");

    // A number chosen, as a constant before its function.
    let at = position("60");
    let end = json!({ "line": at["line"], "character": at["character"].as_u64().unwrap() + 2 });
    let context = json!({ "diagnostics": [], "triggerKind": 1 });
    let params = json!({ "textDocument": { "uri": uri }, "range": { "start": at, "end": end }, "context": context });
    let actions = editor.request("textDocument/codeAction", params);
    let found = actions.as_array().unwrap().iter().find(|a| a["title"] == "Extract into a constant");
    let found = found.unwrap_or_else(|| panic!("{actions}"));
    let text = &applied(&found["edit"], &dir)["src/main.to"];
    assert!(text.contains("const NewConstant = 60\n\npub fn fresh") && text.contains("n * NewConstant"), "{text}");
    editor.stop();
}

/// A hover says where a function can wait, through what it calls: where a
/// cancelled task stops, and where a `defer` of one ends.
#[test]
fn a_hover_says_where_a_function_can_wait() {
    let main = "fn inner(take: Receiver[Int]): Int {\n    take.receive().unwrapOr(0)\n}\n\n\
        fn outer(take: Receiver[Int]): Int {\n    inner(take)\n}\n\nfn main() {}\n";
    let dir = project("hover-waits", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", main)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let at = json!({ "line": 5, "character": 6 });
    let hover = editor.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": at }));
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("Can wait:") && text.contains("waits for a value at src/main.to:2"), "{text}");
}

/// A name after a character of more than a byte, as in a string that is
/// being written, does not end the server.
#[test]
fn completes_after_a_wide_character() {
    let main = "fn main() {\n    let a = \"¤x.\n}\n";
    let dir = project("wide-character", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", main)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let at = json!({ "line": 1, "character": 15 });
    editor.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": at }));
    let hover = editor.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": at }));
    assert!(hover.is_null(), "{hover}");
}

/// Every program of the cases in an editor: asked what an editor asks in
/// places of it, and cut where writing it could have left it. The Nightly's:
/// it takes minutes, and a server that ends loses all that is open.
#[test]
fn the_server_outlives_every_case() {
    if std::env::var_os("TONDO_SWEEP").is_none() {
        return;
    }
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases");
    let mut files: Vec<_> = ["run", "errors"]
        .iter()
        .flat_map(|kind| std::fs::read_dir(cases.join(kind)).unwrap().flatten().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "to"))
        .collect();
    files.sort();
    let mut seed = 1u64;
    let mut next = move |limit: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize % limit.max(1)
    };
    let mut editor = Editor::start();
    for path in &files {
        println!("{}", path.display());
        let text = std::fs::read_to_string(path).unwrap();
        let uri = editor.open(path);
        let document = json!({ "uri": uri });
        let lines: Vec<&str> = text.lines().collect();
        for method in ["foldingRange", "codeLens", "documentLink"] {
            println!("  {method}");
            editor.request(&format!("textDocument/{method}"), json!({ "textDocument": document }));
        }
        for _ in 0..8 {
            let line = next(lines.len());
            let character = next(lines.get(line).map_or(0, |text| text.len()) + 1);
            let at = json!({ "textDocument": document, "position": { "line": line, "character": character } });
            for method in ["hover", "definition", "completion", "signatureHelp", "documentHighlight", "prepareRename"] {
                println!("  {method} {line}:{character}");
                editor.request(&format!("textDocument/{method}"), at.clone());
            }
        }
        for version in 2..6 {
            let mut cut = next(text.len() + 1);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            let written = format!("{}{}", &text[..cut], ["", ".", "(", "{", "\"", "x."][next(6)]);
            let changes = json!([{ "text": written }]);
            let changed = json!({ "textDocument": { "uri": uri, "version": version }, "contentChanges": changes });
            editor.notify("textDocument/didChange", changed);
            let line = written.lines().count().saturating_sub(1);
            let character = written.lines().last().map_or(0, str::len);
            let at = json!({ "textDocument": document, "position": { "line": line, "character": character } });
            println!("  cut at {cut}");
            for method in ["completion", "hover", "signatureHelp"] {
                editor.request(&format!("textDocument/{method}"), at.clone());
            }
            editor.request("textDocument/documentSymbol", json!({ "textDocument": document }));
            editor.request("textDocument/semanticTokens/full", json!({ "textDocument": document }));
        }
        editor.notify("textDocument/didClose", json!({ "textDocument": document }));
    }
}

/// A bug in one request ends that request: what is open stays open.
#[test]
fn the_server_outlives_a_panic_in_a_request() {
    let main = "fn main() {\n    let value = 1\n    _ = value\n}\n";
    let dir = project("survives-panic", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", main)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let failed = editor.request("tondo/panic", json!({}));
    assert_eq!(failed["code"], -32603, "{failed}");
    let at = json!({ "line": 1, "character": 8 });
    let hover = editor.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": at }));
    assert!(hover["contents"]["value"].as_str().unwrap().contains("Int"), "{hover}");
}

/// What a comment accepted stays in sight as a hint, and any other warning
/// offers the comment.
#[test]
fn an_accepted_warning_is_a_hint() {
    let main =
        "// tondo: allow W1004: the name C gives it\ntype CUShort = UInt16\n\ntype CUInt = Int\n\nfn main() {}\n";
    let dir = project("lsp-allow", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", main)]);
    let mut editor = Editor::start();
    let uri = editor.open(&dir.join("src/main.to"));
    let place = json!({ "line": 3, "character": 6 });
    let found = editor.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": { "start": place, "end": place }, "context": { "diagnostics": [] } }),
    );
    let titled = found.as_array().unwrap().iter().any(|a| a["title"] == "accept `W1004`, saying why");
    assert!(titled, "{found}");
    let (_, published) = editor.diagnostics.iter().rev().find(|(path, _)| path.ends_with("main.to")).cloned().unwrap();
    let published = published.as_array().unwrap();
    let hint = published.iter().find(|d| d["severity"] == 4).expect("a hint");
    assert_eq!(hint["code"], "W1004");
    assert!(hint["message"].as_str().unwrap().starts_with("accepted: "), "{hint}");
    assert!(published.iter().any(|d| d["severity"] == 2 && d["code"] == "W1004"), "{published:?}");
    editor.stop();
}

/// Nothing is answered before `initialize` or after `shutdown`, and `exit`
/// with no `shutdown` before it says so in its status.
#[test]
fn it_keeps_to_the_start_and_the_end_of_a_session() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut said = |message: Value| {
        let text = message.to_string();
        write!(stdin, "Content-Length: {}\r\n\r\n{text}", text.len()).unwrap();
    };
    said(json!({ "jsonrpc": "2.0", "id": 1, "method": "textDocument/hover", "params": {} }));
    said(json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "capabilities": {} } }));
    said(json!({ "jsonrpc": "2.0", "method": "exit" }));
    drop(stdin);
    let run = child.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(out.contains("-32002"), "{out}");
    assert!(!run.status.success(), "{out}");

    let mut editor = Editor::start();
    assert_eq!(editor.request("shutdown", Value::Null), Value::Null);
    let refused = editor.request("textDocument/hover", json!({}));
    assert_eq!(refused["code"], -32600, "{refused}");
    editor.notify("exit", Value::Null);
    assert!(editor.child.wait().unwrap().success());
}

/// A server on a file with a snippet, a documented function and a symbol in a
/// type, for an editor that says only what is given.
fn session(name: &str, capabilities: Value) -> (Editor, String) {
    let dir = project(
        name,
        &[(
            "main.to",
            "/// Says hello.\nfn hello() {}\n\ntype Door = {\n    open: Bool\n}\n\nfn main() {\n    hello()\n}\n",
        )],
    );
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("main.to"));
    (editor, uri)
}

/// An answer fits what the editor said it takes.
#[test]
fn answers_fit_what_the_editor_takes() {
    let (mut editor, uri) = session("lsp-fit", json!({}));
    let at = json!({ "textDocument": { "uri": uri }, "position": { "line": 8, "character": 6 } });
    let hover = editor.request("textDocument/hover", at);
    assert_eq!(hover["contents"]["kind"], "plaintext", "{hover}");
    let symbols = editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    let names: Vec<&str> = symbols.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"open") && symbols[0].get("location").is_some(), "{symbols}");
    let line = json!({ "textDocument": { "uri": uri }, "position": { "line": 8, "character": 4 } });
    let completion = editor.request("textDocument/completion", line);
    let items = completion["items"].as_array().unwrap();
    assert!(!items.is_empty() && items.iter().all(|item| item["insertTextFormat"] != 2), "{completion}");
    assert!(items.iter().all(|item| item.get("labelDetails").is_none()), "{completion}");
    editor.stop();
}

/// A request the editor gave up on is not answered with its work, and neither
/// is one of a whole document with a change behind it.
#[test]
fn a_request_can_be_given_up_on() {
    let (mut editor, uri) = session("lsp-given-up", json!({}));
    editor.notify("$/cancelRequest", json!({ "id": editor.next + 1 }));
    let refused = editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(refused["code"], -32800, "{refused}");
    // The next change is read before the request's turn comes.
    let change = json!({
        "textDocument": { "uri": uri, "version": 2 },
        "contentChanges": [{ "text": "fn main() {}\n" }],
    });
    editor.next += 2;
    let (hold, asked) = (editor.next - 1, editor.next);
    editor.send(json!({ "jsonrpc": "2.0", "id": hold, "method": "tondo/hold", "params": { "uri": uri } }));
    editor.send(json!({ "jsonrpc": "2.0", "id": asked, "method": "textDocument/semanticTokens/full",
        "params": { "textDocument": { "uri": uri } } }));
    editor.notify("textDocument/didChange", change);
    let answer = loop {
        let message = editor.receive();
        if message["id"] == asked {
            break message;
        }
    };
    assert_eq!(answer["error"]["code"], -32801, "{answer}");
    editor.stop();
}

/// The editor is told how the first analysis goes, how long each request
/// took if it asked, and of a request's own progress.
#[test]
fn says_how_work_goes() {
    let capabilities = json!({ "window": { "workDoneProgress": true } });
    let dir =
        project("lsp-progress", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", "fn main() {}\n")]);
    let mut editor = Editor::launch_with(
        capabilities,
        json!({}),
        json!([{ "uri": format!("file://{}", dir.display()) }]),
        "messages",
    );
    let create = loop {
        let message = editor.receive();
        if message["method"] == "window/workDoneProgress/create" {
            break message;
        }
    };
    editor.send(json!({ "jsonrpc": "2.0", "id": create["id"], "result": null }));
    let token = create["params"]["token"].clone();
    let mut kinds = Vec::new();
    while kinds.last().is_none_or(|kind| kind != "end") {
        let message = editor.receive();
        if message["method"] == "$/progress" && message["params"]["token"] == token {
            kinds.push(message["params"]["value"]["kind"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(kinds, ["begin", "report", "end"]);
    let (mut progress, mut traced) = (Vec::new(), false);
    editor.next += 1;
    let id = editor.next;
    editor.send(json!({ "jsonrpc": "2.0", "id": id, "method": "workspace/symbol",
        "params": { "query": "main", "workDoneToken": "mine" } }));
    loop {
        let message = editor.receive();
        if message["method"] == "$/progress" {
            progress.push(message["params"]["token"].clone());
        }
        traced |= message["method"] == "$/logTrace";
        if message["id"] == id {
            break;
        }
    }
    assert_eq!(progress, ["mine", "mine"]);
    assert!(traced);
    editor.stop();
}

/// A change says only what changed, in the units the editor counts.
#[test]
fn a_change_says_only_what_changed() {
    let (mut editor, uri) = session("lsp-change", everything());
    let change = |line: u32, from: u32, to: u32, text: &str, version: u32| {
        json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [{
                "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
                "text": text,
            }],
        })
    };
    // `hello()` in `main` becomes an error, then back again in two parts.
    editor.notify("textDocument/didChange", change(8, 4, 9, "goodbye", 2));
    let symbols = editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    assert!(symbols.is_array(), "{symbols}");
    let (version, diagnostics) = editor.published(&uri);
    assert_eq!(version, 2);
    assert!(diagnostics.to_string().contains("goodbye"), "{diagnostics}");
    editor.notify("textDocument/didChange", change(8, 4, 11, "hel", 3));
    editor.notify("textDocument/didChange", change(8, 7, 7, "lo", 4));
    let _ = editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    let (version, diagnostics) = editor.published(&uri);
    assert_eq!((version, diagnostics), (4, json!([])));
    editor.stop();
}

/// Settings come from the editor, and saving formats only while they say so,
/// which an editor that can registers and unregisters.
#[test]
fn the_settings_of_the_editor_hold() {
    let mut capabilities = everything();
    capabilities["workspace"] = json!({
        "configuration": true,
        "didChangeConfiguration": { "dynamicRegistration": true },
    });
    capabilities["textDocument"]["synchronization"] = json!({ "dynamicRegistration": true });
    let dir = project("lsp-settings", &[("main.to", "fn main( ) {\n    let total = 1\n    _ = total\n}\n")]);
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let settings = |editor: &mut Editor, value: Value| {
        let asked = editor.receive_method("workspace/configuration");
        assert_eq!(asked["params"]["items"][0]["section"], "tondo.lsp");
        editor.send(json!({ "jsonrpc": "2.0", "id": asked["id"], "result": [value] }));
    };
    settings(&mut editor, json!({ "formatOnSave": true, "inlayHints": false }));
    let registered = editor.receive_method("client/registerCapability");
    let registered = registered["params"]["registrations"].to_string();
    assert!(registered.contains("willSaveWaitUntil"), "{registered}");
    let uri = editor.open(&dir.join("main.to"));
    let document = json!({ "textDocument": { "uri": uri }, "reason": 1 });
    let edits = editor.request("textDocument/willSaveWaitUntil", document.clone());
    assert_eq!(edits.as_array().unwrap().len(), 1, "{edits}");
    let everywhere = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 9, "character": 0 } });
    let hints =
        editor.request("textDocument/inlayHint", json!({ "textDocument": { "uri": uri }, "range": everywhere }));
    assert_eq!(hints, json!([]));
    editor.notify("workspace/didChangeConfiguration", json!({ "settings": null }));
    settings(&mut editor, json!({ "formatOnSave": false }));
    let gone = editor.receive_method("client/unregisterCapability");
    assert!(gone["params"]["unregisterations"].to_string().contains("willSaveWaitUntil"), "{gone}");
    assert_eq!(editor.request("textDocument/willSaveWaitUntil", document), json!([]));
    let hints =
        editor.request("textDocument/inlayHint", json!({ "textDocument": { "uri": uri }, "range": everywhere }));
    assert!(!hints.as_array().unwrap().is_empty(), "{hints}");
    editor.stop();
}

/// A folder that comes is analyzed, one that goes leaves no diagnostics, and a
/// file the editor makes or deletes counts at once.
#[test]
fn folders_and_files_come_and_go() {
    let dir = project(
        "lsp-folders",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", "fn main() {\n    missing()\n}\n")],
    );
    let mut editor = Editor::launch_with(everything(), json!({}), json!([]), "off");
    let folder = json!({ "uri": format!("file://{}", dir.display()), "name": "app" });
    let main = format!("file://{}/src/main.to", dir.display());
    let event = json!({ "event": { "added": [folder], "removed": [] } });
    editor.notify("workspace/didChangeWorkspaceFolders", event);
    let found = editor.receive_diagnostics(&main);
    assert!(found.to_string().contains("missing"), "{found}");
    let write = |text: &str| std::fs::write(dir.join("src/main.to"), text).unwrap();
    write("fn main() {}\n");
    let files = json!({ "files": [{ "uri": main }] });
    assert_eq!(editor.request("workspace/willCreateFiles", files.clone()), Value::Null);
    editor.notify("workspace/didCreateFiles", files.clone());
    assert_eq!(editor.receive_diagnostics(&main), json!([]));
    write("fn main() {\n    missing()\n}\n");
    editor.notify("workspace/didRenameFiles", json!({ "files": [{ "oldUri": main, "newUri": main }] }));
    assert!(editor.receive_diagnostics(&main).to_string().contains("missing"));
    let event = json!({ "event": { "added": [], "removed": [folder_of(&dir)] } });
    editor.notify("workspace/didChangeWorkspaceFolders", event);
    assert_eq!(editor.receive_diagnostics(&main), json!([]));
    editor.stop();
}

fn folder_of(dir: &Path) -> Value {
    json!({ "uri": format!("file://{}", dir.display()), "name": "app" })
}

/// What the server does in the editor, as it says it can: an edit of a
/// refactor, the views it shows, and the place a test failed.
#[test]
fn asks_the_editor_to_act() {
    let mut capabilities = everything();
    capabilities["workspace"] = json!({
        "applyEdit": true,
        "semanticTokens": { "refreshSupport": true },
        "inlayHint": { "refreshSupport": true },
        "workspaceFolders": true,
    });
    capabilities["window"] = json!({ "showDocument": { "support": true }, "showMessage": {} });
    let dir = project(
        "lsp-acts",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", "fn add(a: Int, b: Int): Int {\n    a + b\n}\n\nfn main() {\n    _ = add(1, 2)\n}\n"),
            ("src/main_test.to", "test wrong {\n    assert(1 == 2)\n}\n"),
        ],
    );
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let asked = editor.receive_method("workspace/workspaceFolders");
    editor.send(json!({ "jsonrpc": "2.0", "id": asked["id"], "result": [folder_of(&dir)] }));
    let main = editor.open(&dir.join("src/main.to"));

    let at = json!({ "line": 0, "character": 3 });
    let signature = json!({ "command": "tondo.signature", "arguments": [main, at, "b: Int, a: Int"] });
    editor.next += 1;
    let id = editor.next;
    editor.send(json!({ "jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand", "params": signature }));
    let apply = editor.receive_method("workspace/applyEdit");
    assert!(apply["params"]["edit"]["changes"].to_string().contains("b: Int, a: Int"), "{apply}");
    editor.send(json!({ "jsonrpc": "2.0", "id": apply["id"], "result": { "applied": true } }));

    editor.notify("workspace/didChangeWatchedFiles", json!({ "changes": [{ "uri": main, "type": 2 }] }));
    let refresh = editor.receive_method("workspace/semanticTokens/refresh");
    editor.send(json!({ "jsonrpc": "2.0", "id": refresh["id"], "result": null }));
    let refresh = editor.receive_method("workspace/inlayHint/refresh");
    editor.send(json!({ "jsonrpc": "2.0", "id": refresh["id"], "result": null }));

    let run = json!({ "command": "tondo.runTest", "arguments": [main, "app::unit::app::wrong"] });
    editor.request("workspace/executeCommand", run);
    let failed = editor.receive_method("window/showMessageRequest");
    assert!(failed["params"]["message"].to_string().contains("failed"), "{failed}");
    let chosen = failed["params"]["actions"][0].clone();
    editor.send(json!({ "jsonrpc": "2.0", "id": failed["id"], "result": chosen }));
    let shown = editor.receive_method("window/showDocument");
    assert!(shown["params"]["uri"].as_str().unwrap().ends_with("src/main_test.to"), "{shown}");
    assert_eq!(shown["params"]["selection"]["start"]["line"], 1, "{shown}");
    editor.send(json!({ "jsonrpc": "2.0", "id": shown["id"], "result": { "success": true } }));
    editor.stop();
}

/// A client with no literals for code actions gets commands, which apply.
#[test]
fn code_actions_go_as_commands_where_they_must() {
    let mut capabilities = everything();
    capabilities["workspace"] = json!({ "applyEdit": true });
    capabilities["textDocument"]["codeAction"] = json!({});
    let dir = project("lsp-commands", &[("main.to", "fn main() {\n    let unused = 1\n}\n")]);
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("main.to"));
    let start = json!({ "line": 1, "character": 8 });
    let actions = editor.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": { "start": start, "end": start }, "context": { "diagnostics": [] } }),
    );
    let command = actions.as_array().unwrap().iter().find(|action| action["command"] == "tondo.apply");
    let command = command.unwrap_or_else(|| panic!("{actions}"));
    assert!(command.get("kind").is_none(), "{command}");
    let run = json!({ "command": "tondo.apply", "arguments": command["arguments"] });
    editor.next += 1;
    let id = editor.next;
    editor.send(json!({ "jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand", "params": run }));
    let apply = editor.receive_method("workspace/applyEdit");
    assert!(apply["params"]["edit"].to_string().contains("_unused"), "{apply}");
    editor.stop();
}

/// What an editor can ask for later is left out of a list until it does.
#[test]
fn items_are_filled_in_when_asked() {
    let mut capabilities = everything();
    let properties = |names: Value| json!({ "properties": names });
    capabilities["textDocument"]["completion"]["completionItem"]["resolveSupport"] =
        properties(json!(["documentation"]));
    capabilities["textDocument"]["codeAction"]["resolveSupport"] = properties(json!(["edit"]));
    capabilities["textDocument"]["codeLens"] = json!({ "resolveSupport": properties(json!(["command"])) });
    capabilities["textDocument"]["inlayHint"] =
        json!({ "resolveSupport": properties(json!(["tooltip", "textEdits"])) });
    capabilities["textDocument"]["documentLink"] = json!({ "tooltipSupport": true });
    capabilities["workspace"] = json!({ "symbol": { "resolveSupport": properties(json!(["location.range"])) } });
    let dir = project(
        "lsp-resolve",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/more/more.to", "pub fn twice(n: Int): Int {\n    n * 2\n}\n"),
            (
                "src/main.to",
                "import app.more\n\n/// Adds one.\nfn next(n: Int): Int {\n    n + 1\n}\n\nfn main() {\n    let unused = next(more.twice(1))\n}\n",
            ),
        ],
    );
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("src/main.to"));
    let document = json!({ "uri": uri });

    let at = json!({ "textDocument": document, "position": { "line": 8, "character": 19 } });
    let list = editor.request("textDocument/completion", at);
    let items = list["items"].as_array().unwrap();
    let next = items.iter().find(|item| item["label"] == "next").unwrap_or_else(|| panic!("{list}"));
    assert!(next.get("documentation").is_none() && next.get("data").is_some(), "{next}");
    let filled = editor.request("completionItem/resolve", next.clone());
    assert!(filled["documentation"].to_string().contains("Adds one"), "{filled}");
    assert!(filled.get("data").is_none(), "{filled}");

    let line = json!({ "line": 8, "character": 8 });
    let context = json!({ "diagnostics": [] });
    let range = json!({ "start": line, "end": line });
    let actions = json!({ "textDocument": document, "range": range, "context": context });
    let actions = editor.request("textDocument/codeAction", actions);
    let action =
        actions.as_array().unwrap().iter().find(|a| a.get("data").is_some()).unwrap_or_else(|| panic!("{actions}"));
    assert!(action.get("edit").is_none(), "{action}");
    assert!(editor.request("codeAction/resolve", action.clone()).get("edit").is_some());

    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": document }));
    let lens =
        lenses.as_array().unwrap().iter().find(|l| l.get("data").is_some()).unwrap_or_else(|| panic!("{lenses}"));
    assert!(lens.get("command").is_none(), "{lens}");
    assert!(editor.request("codeLens/resolve", lens.clone()).get("command").is_some());

    let all = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 11, "character": 0 } });
    let hints = editor.request("textDocument/inlayHint", json!({ "textDocument": document, "range": all }));
    let hint = hints.as_array().unwrap().iter().find(|h| h.get("data").is_some()).unwrap_or_else(|| panic!("{hints}"));
    assert!(hint.get("textEdits").is_none(), "{hint}");
    let hint = editor.request("inlayHint/resolve", hint.clone());
    assert_eq!(hint["textEdits"][0]["newText"], hint["label"], "{hint}");

    let links = editor.request("textDocument/documentLink", json!({ "textDocument": document }));
    assert!(links[0].get("tooltip").is_none(), "{links}");
    let link = editor.request("documentLink/resolve", links[0].clone());
    assert!(link["tooltip"].to_string().contains("more"), "{link}");

    let symbols = editor.request("workspace/symbol", json!({ "query": "next" }));
    assert!(symbols[0]["location"].get("range").is_none(), "{symbols}");
    let symbol = editor.request("workspaceSymbol/resolve", symbols[0].clone());
    assert!(symbol["location"].get("range").is_some(), "{symbol}");
    editor.stop();
}

/// A client that asks for diagnostics gets them for the workspace, and hears
/// only of what changed; semantic tokens go the same way.
#[test]
fn only_what_changed_is_said_again() {
    let dir = project(
        "lsp-pull",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", "fn main() {\n    missing()\n}\n")],
    );
    let folders = json!([folder_of(&dir)]);
    let mut editor = Editor::launch_with(everything(), json!({ "pullDiagnostics": true }), folders, "off");
    let main = editor.open(&dir.join("src/main.to"));
    let document = json!({ "textDocument": { "uri": main } });
    editor.request("textDocument/documentSymbol", document.clone());
    let report = editor.request("workspace/diagnostic", json!({ "previousResultIds": [] }));
    let item = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["uri"] == main)
        .unwrap_or_else(|| panic!("{report}"));
    assert_eq!(item["kind"], "full", "{item}");
    assert!(item["items"].to_string().contains("missing"), "{item}");
    let known = json!([{ "uri": main, "value": item["resultId"] }]);
    let again = editor.request("workspace/diagnostic", json!({ "previousResultIds": known }));
    assert_eq!(again["items"][0]["kind"], "unchanged", "{again}");
    let previous = json!({ "textDocument": { "uri": main }, "previousResultId": item["resultId"] });
    assert_eq!(editor.request("textDocument/diagnostic", previous)["kind"], "unchanged");

    let full = editor.request("textDocument/semanticTokens/full", document.clone());
    let (id, data) = (full["resultId"].clone(), full["data"].as_array().unwrap().clone());
    assert!(!data.is_empty(), "{full}");
    let same = json!({ "textDocument": { "uri": main }, "previousResultId": id });
    let delta = editor.request("textDocument/semanticTokens/full/delta", same);
    assert_eq!(delta["edits"][0]["deleteCount"], 0, "{delta}");
    assert_eq!(delta["edits"][0]["data"], json!([]), "{delta}");
    let change = json!({
        "textDocument": { "uri": main, "version": 2 },
        "contentChanges": [{ "text": "fn main() {\n    let more = 1\n    missing(more)\n}\n" }],
    });
    editor.notify("textDocument/didChange", change);
    let next = json!({ "textDocument": { "uri": main }, "previousResultId": delta["resultId"] });
    let delta = editor.request("textDocument/semanticTokens/full/delta", next);
    let edit = &delta["edits"][0];
    let (start, deleted) = (edit["start"].as_u64().unwrap() as usize, edit["deleteCount"].as_u64().unwrap() as usize);
    let mut patched = data.clone();
    patched.splice(start..start + deleted, edit["data"].as_array().unwrap().iter().cloned());
    let whole = editor.request("textDocument/semanticTokens/full", document);
    assert_eq!(Value::Array(patched), whole["data"], "{delta}");
    let lost = json!({ "textDocument": { "uri": main }, "previousResultId": "gone" });
    assert!(editor.request("textDocument/semanticTokens/full/delta", lost).get("data").is_some());
    editor.stop();
}

/// What is optional in an answer goes to the editor that takes it.
#[test]
fn optional_fields_go_where_they_are_taken() {
    let mut capabilities = everything();
    let document = &mut capabilities["textDocument"];
    document["definition"] = json!({ "linkSupport": true });
    document["codeAction"]["disabledSupport"] = json!(true);
    document["foldingRange"] = json!({ "foldingRange": { "collapsedText": true } });
    document["completion"]["completionItem"]["commitCharactersSupport"] = json!(true);
    document["completion"]["completionList"] = json!({ "itemDefaults": ["commitCharacters"] });
    capabilities["workspace"] = json!({
        "workspaceEdit": { "documentChanges": true, "changeAnnotationSupport": {} },
    });
    let dir = project(
        "lsp-optional",
        &[(
            "main.to",
            "// one\n// two\nfn add(a: Int): Int {\n    a + 1\n}\n\nfn main() {\n    _ = add(1)\n    ad\n}\n",
        )],
    );
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("main.to"));
    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });

    let link = editor.request("textDocument/definition", at(7, 9));
    assert!(link[0].get("targetUri").is_some(), "{link}");

    let start = json!({ "line": 0, "character": 0 });
    let invoked = json!({ "textDocument": { "uri": uri }, "range": { "start": start, "end": start },
        "context": { "diagnostics": [], "triggerKind": 1 } });
    let actions = editor.request("textDocument/codeAction", invoked);
    assert!(actions.to_string().contains("the imports are in order"), "{actions}");

    let folds = editor.request("textDocument/foldingRange", json!({ "textDocument": { "uri": uri } }));
    assert!(folds.to_string().contains("collapsedText"), "{folds}");

    let list = editor.request("textDocument/completion", at(8, 6));
    assert_eq!(list["itemDefaults"]["commitCharacters"], json!(["."]), "{list}");
    let items = list["items"].as_array().unwrap();
    let add = items.iter().find(|item| item["label"] == "add").unwrap_or_else(|| panic!("{list}"));
    assert_eq!(add["commitCharacters"], json!(["("]), "{add}");

    let mut rename = at(7, 9);
    rename["newName"] = json!("sum");
    let edit = editor.request("textDocument/rename", rename);
    assert_eq!(edit["changeAnnotations"]["rename"]["label"], "Rename in 1 file", "{edit}");
    assert_eq!(edit["documentChanges"][0]["edits"][0]["annotationId"], "rename", "{edit}");

    let range =
        |line: u32| json!({ "start": { "line": line, "character": 0 }, "end": { "line": line + 1, "character": 0 } });
    let ranges = json!({ "textDocument": { "uri": uri }, "ranges": [range(0), range(3)], "options": {} });
    assert!(editor.request("textDocument/rangesFormatting", ranges).is_array());
    editor.stop();
}

/// The names that hold across projects, what is sure to come next, the
/// standard modules through the server, and how long an analysis took.
#[test]
fn what_the_protocol_adds_at_the_end() {
    let dir = project(
        "lsp-last",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            (
                "src/main.to",
                "import std.console\nimport std.model\n\npub fn greeting(): String {\n    let word = \"hello\"\n    word\n}\n\nfn main() {\n    console.println(greeting())\n    let step = model.Step { name: \"a\", state: 1 }\n    console.println(step.name)\n}\n",
            ),
        ],
    );
    let options = json!({ "virtualStandardLibrary": true });
    let mut editor = Editor::launch_with(everything(), options, Value::Null, "off");
    let uri = editor.open(&dir.join("src/main.to"));
    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });

    let export = editor.request("textDocument/moniker", at(3, 8));
    assert_eq!((&export[0]["kind"], &export[0]["unique"]), (&json!("export"), &json!("project")), "{export}");
    assert!(export[0]["identifier"].as_str().unwrap().ends_with(".greeting"), "{export}");
    let local = editor.request("textDocument/moniker", at(5, 5));
    assert_eq!((&local[0]["kind"], &local[0]["unique"]), (&json!("local"), &json!("document")), "{local}");
    let import = editor.request("textDocument/moniker", at(10, 22));
    assert_eq!(import[0]["kind"], "import", "{import}");

    let mut inline = at(9, 25);
    inline["context"] = json!({ "triggerKind": 2 });
    let inline = editor.request("textDocument/inlineCompletion", inline);
    assert_eq!(inline["items"][0]["insertText"], "greeting", "{inline}");
    assert_eq!(inline["items"][0]["range"]["start"]["character"], 20, "{inline}");
    let mut nothing = at(9, 20);
    nothing["context"] = json!({ "triggerKind": 2 });
    assert_eq!(editor.request("textDocument/inlineCompletion", nothing)["items"], json!([]));

    let found = editor.request("textDocument/definition", at(10, 22));
    let target = found["uri"].as_str().or(found[0]["uri"].as_str()).unwrap_or_else(|| panic!("{found}"));
    assert!(target.starts_with("tondo-std:/"), "{target}");
    let content = editor.request("workspace/textDocumentContent", json!({ "uri": target }));
    assert!(content["text"].as_str().unwrap().contains("pub type Step"), "{content}");
    let outside = editor.request("workspace/textDocumentContent", json!({ "uri": "tondo-std:/../../secret" }));
    assert_eq!(outside["code"], -32602, "{outside}");
    assert_eq!(editor.request("workspace/textDocumentContent", json!({ "uri": uri }))["code"], -32602);
    editor.stop();
}

/// An analysis says what it took, to an editor that collects that.
#[test]
fn an_analysis_says_what_it_took() {
    let (mut editor, _) = session("lsp-telemetry", everything());
    let event = editor.receive_method("telemetry/event");
    assert_eq!(event["params"]["name"], "analysis", "{event}");
    assert!(event["params"]["files"].as_u64().unwrap() >= 1, "{event}");
    editor.stop();
}

/// The first analysis can be cancelled, and what it left comes when asked.
#[test]
fn the_first_analysis_can_be_cancelled() {
    let dir = project(
        "lsp-cancel-analysis",
        &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", "fn main() {\n    missing()\n}\n")],
    );
    let capabilities = json!({ "window": { "workDoneProgress": true } });
    let mut editor = Editor::launch_with(capabilities, json!({}), json!([folder_of(&dir)]), "off");
    let create = editor.receive_method("window/workDoneProgress/create");
    let token = create["params"]["token"].clone();
    editor.notify("window/workDoneProgress/cancel", json!({ "token": token }));
    editor.send(json!({ "jsonrpc": "2.0", "id": create["id"], "result": null }));
    let begin = editor.receive_method("$/progress");
    assert_eq!(begin["params"]["value"]["cancellable"], true, "{begin}");
    let end = editor.receive_method("$/progress");
    assert_eq!(end["params"]["value"]["kind"], "end", "{end}");
    let uri = editor.open(&dir.join("src/main.to"));
    assert!(editor.receive_diagnostics(&uri).to_string().contains("missing"));
    editor.stop();
}

/// What is pending is analyzed before a save, and an editor that watches
/// nothing has what a save changes analyzed.
#[test]
fn a_save_is_seen() {
    let (mut editor, uri) = session("lsp-save", everything());
    let change = json!({
        "textDocument": { "uri": uri, "version": 2 },
        "contentChanges": [{ "text": "fn main() {\n    wrong()\n}\n" }],
    });
    editor.notify("textDocument/didChange", change);
    editor.notify("textDocument/willSave", json!({ "textDocument": { "uri": uri }, "reason": 1 }));
    assert!(editor.receive_diagnostics(&uri).to_string().contains("wrong"));
    editor.notify("textDocument/didSave", json!({ "textDocument": { "uri": uri } }));
    assert!(editor.receive_diagnostics(&uri).to_string().contains("wrong"));
    editor.stop();
}

/// An editor with no tests of its own has a lens for every order of a test.
#[test]
fn every_order_of_a_test_is_a_lens() {
    let dir = project(
        "lsp-orders",
        &[
            ("tondo.toml", "[package]\nname = \"app\"\n"),
            ("src/main.to", "fn main() {}\n"),
            ("src/main_test.to", "test holds {\n    assert(1 == 1)\n}\n"),
        ],
    );
    let mut capabilities = everything();
    capabilities["window"] = json!({ "showMessage": {} });
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("src/main_test.to"));
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let every = lenses.as_array().unwrap().iter().find(|lens| lens["command"]["title"] == "Every order");
    let every = every.unwrap_or_else(|| panic!("{lenses}"));
    let run = json!({ "command": every["command"]["command"], "arguments": every["command"]["arguments"] });
    editor.request("workspace/executeCommand", run);
    let said = editor.receive_method("window/showMessage");
    assert!(said["params"]["message"].to_string().contains("all that differ"), "{said}");
    editor.stop();
}

/// What is deprecated shows so before it is used.
#[test]
fn what_is_deprecated_shows_before_it_is_used() {
    let text = "@deprecated(\"use `fresh`\")\nfn stale() {}\n\nfn fresh() {}\n\nfn main() {\n    fresh()\n    st\n}\n";
    let dir = project("lsp-deprecated", &[("main.to", text)]);
    let mut editor = Editor::launch_with(everything(), json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("main.to"));
    let symbols = editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    let named = |list: &Value, name: &str, key: &str| {
        list.as_array().unwrap().iter().find(|item| item[key] == name).unwrap_or_else(|| panic!("{list}")).clone()
    };
    assert_eq!(named(&symbols, "stale", "name")["tags"], json!([1]), "{symbols}");
    assert_eq!(named(&symbols, "fresh", "name")["tags"], json!([]), "{symbols}");
    let at = json!({ "textDocument": { "uri": uri }, "position": { "line": 7, "character": 6 } });
    let list = editor.request("textDocument/completion", at);
    let items = &list["items"];
    assert_eq!(named(items, "stale", "label")["tags"], json!([1]), "{list}");
    assert!(named(items, "fresh", "label").get("tags").is_none(), "{list}");
    let found = editor.request("workspace/symbol", json!({ "query": "stale" }));
    assert_eq!(found[0]["tags"], json!([1]), "{found}");
    editor.stop();
}

/// What an `impl` lacks is written one method at a time, or all at once.
#[test]
fn an_impl_writes_what_it_lacks() {
    let text = "trait Shape {\n    fn area(self): Float\n    fn sides(self): Int\n}\n\ntype Square = {\n    side: Float\n}\n\nimpl Shape for Square {\n}\n\nfn main() {}\n";
    let dir = project("lsp-impl", &[("main.to", text)]);
    let mut editor = Editor::launch_with(everything(), json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("main.to"));
    let at = json!({ "line": 9, "character": 0 });
    let asked = json!({ "textDocument": { "uri": uri }, "range": { "start": at, "end": at }, "context": { "diagnostics": [] } });
    let actions = editor.request("textDocument/codeAction", asked);
    let all = actions.as_array().unwrap().iter().find(|action| action["title"] == "write the 2 methods missing");
    let all = all.unwrap_or_else(|| panic!("{actions}"));
    let written = all["edit"].to_string();
    assert!(written.contains("fn area(self): Float") && written.contains("fn sides(self): Int"), "{written}");
    let edit = &all["edit"]["changes"][&uri][0];
    let at = edit["range"]["start"]["line"].as_u64().unwrap() as usize;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    lines.insert(at, edit["newText"].as_str().unwrap().trim_end().to_string());
    let change = json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": lines.join("\n") + "\n" }] });
    editor.notify("textDocument/didChange", change);
    editor.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    let (_, diagnostics) = editor.published(&uri);
    assert!(!diagnostics.to_string().contains("E1114"), "{diagnostics}");
    editor.stop();
}

/// A request can be given up on while it waits for an analysis, which then
/// leaves the projects it did not reach for later.
#[test]
fn a_request_can_be_cancelled_while_it_runs() {
    let file = ("src/main.to", "fn main() {\n    missing()\n}\n");
    let toml = ("tondo.toml", "[package]\nname = \"app\"\n");
    let (first, second) = (project("lsp-running-a", &[toml, file]), project("lsp-running-b", &[toml, file]));
    let mut editor = Editor::launch_with(everything(), json!({}), Value::Null, "off");
    let hold = json!({ "jsonrpc": "2.0", "id": "hold", "method": "tondo/hold", "params": { "analysis": true } });
    editor.send(hold);
    let (a, b) = (first.join("src/main.to"), second.join("src/main.to"));
    let (a, b) = (format!("file://{}", a.display()), format!("file://{}", b.display()));
    for uri in [&a, &b] {
        let document = json!({ "uri": uri, "languageId": "tondo", "version": 1, "text": file.1 });
        editor.notify("textDocument/didOpen", json!({ "textDocument": document }));
    }
    editor.send(json!({ "jsonrpc": "2.0", "id": 900, "method": "workspace/symbol", "params": { "query": "main" } }));
    // The first project's diagnostics say the analysis has started.
    let reached = loop {
        let message = editor.receive_method("textDocument/publishDiagnostics");
        if message["params"]["uri"] == a || message["params"]["uri"] == b {
            break message["params"]["uri"].as_str().unwrap().to_string();
        }
    };
    editor.notify("$/cancelRequest", json!({ "id": 900 }));
    let answer = loop {
        let message = editor.receive();
        if message["id"] == 900 {
            break message;
        }
    };
    assert_eq!(answer["error"]["code"], -32800, "{answer}");
    // The other project is analyzed all the same.
    let other = if reached == a { b } else { a };
    assert!(editor.receive_diagnostics(&other).to_string().contains("missing"));
}

/// A long list goes as it is sent, to an editor that gave a token for that.
#[test]
fn a_long_answer_comes_in_parts() {
    let functions: String = (0..250).map(|n| format!("fn job{n}() {{}}\n\n")).collect();
    let dir = project("lsp-parts", &[("main.to", &format!("{functions}fn main() {{}}\n"))]);
    let mut editor = Editor::launch_with(everything(), json!({}), Value::Null, "off");
    editor.open(&dir.join("main.to"));
    editor.next += 1;
    let id = editor.next;
    editor.send(json!({ "jsonrpc": "2.0", "id": id, "method": "workspace/symbol",
        "params": { "query": "job", "partialResultToken": "parts" } }));
    let mut parts = Vec::new();
    let answer = loop {
        let message = editor.receive();
        if message["method"] == "$/progress" && message["params"]["token"] == "parts" {
            parts.push(message["params"]["value"].as_array().unwrap().len());
        }
        if message["id"] == id {
            break message;
        }
    };
    // Those of the program, and any of the standard library with that name.
    assert!(parts[0] == 100 && parts.iter().sum::<usize>() >= 250, "{parts:?}");
    assert_eq!(answer["result"], json!([]), "{answer}");
    editor.stop();
}

/// Over a function that is a state machine, a lens checks it with no model to
/// write, and says how it went.
#[test]
fn a_state_machine_has_a_lens() {
    let program = "pub enum Event {\n    Open\n    Close\n}\n\npub fn apply(open: Bool, event: Event): Bool {\n    match event {\n        \
Event.Open => true\n        Event.Close => false\n    }\n}\n\nfn main() {}\n";
    let dir = project("lsp-machine", &[("tondo.toml", "[package]\nname = \"app\"\n"), ("src/main.to", program)]);
    let mut capabilities = everything();
    capabilities["window"] = json!({ "showMessage": {} });
    let mut editor = Editor::launch_with(capabilities, json!({}), Value::Null, "off");
    let uri = editor.open(&dir.join("src/main.to"));
    let lenses = editor.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let lens = lenses.as_array().unwrap().iter().find(|lens| lens["command"]["command"] == "tondo.checkMachine");
    let lens = lens.unwrap_or_else(|| panic!("{lenses}"));
    assert_eq!(lens["command"]["arguments"][1], "apply", "{lens}");
    let run = json!({ "command": "tondo.checkMachine", "arguments": lens["command"]["arguments"] });
    editor.request("workspace/executeCommand", run);
    let said = editor.receive_method("window/showMessage");
    assert!(said["params"]["message"].to_string().contains("holds as a state machine in 2 states"), "{said}");
    assert!(dir.join("src/main_test.to").exists());
    editor.stop();
}
