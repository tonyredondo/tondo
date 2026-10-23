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
}

impl Editor {
    fn start() -> Editor {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut editor = Editor { child, input, output, next: 0, diagnostics: Vec::new() };
        let initialized =
            editor.request("initialize", json!({ "capabilities": { "general": { "positionEncodings": ["utf-8"] } } }));
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
    let dir = project("lsp-diagnostics", &[("main.to", "fn main() {\n    let x: Int = \"text\"\n}\n")]);
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
    assert_eq!(labels, [": String", ": models.User", ": Int"], "{hints}");
    editor.stop();
}
