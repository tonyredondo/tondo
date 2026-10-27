//! `tondo mcp` (toolchain 7.9) as an agent uses it: JSON-RPC a line, over
//! standard input and output, in both eras of the protocol.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Agent {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Agent {
    fn start(dir: &Path) -> Agent {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("mcp")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Agent { child, input, output, next: 0 }
    }

    /// The whole response to a request.
    fn send(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let message = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], self.next);
        response
    }

    /// A modern request's result.
    fn request(&mut self, method: &str, mut params: Value) -> Value {
        params["_meta"] = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28" });
        let response = self.send(method, params);
        assert_eq!(response["result"]["resultType"], "complete", "{response}");
        response["result"].clone()
    }

    /// A tool's structured result and its text.
    fn call(&mut self, tool: &str, arguments: Value) -> (Value, String) {
        let result = self.request("tools/call", json!({ "name": tool, "arguments": arguments }));
        assert_eq!(result["isError"], false, "{tool}: {result}");
        (result["structuredContent"].clone(), result["content"][0]["text"].as_str().unwrap().to_string())
    }

    fn stop(mut self) {
        drop(self.input);
        assert!(self.child.wait().unwrap().success());
    }
}

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir.canonicalize().unwrap()
}

#[test]
fn both_eras_of_the_protocol() {
    let dir = project("mcp_eras", &[]);
    let mut agent = Agent::start(&dir);

    // Modern: no handshake; each request names its version.
    let discovered = agent.request("server/discover", json!({}));
    assert_eq!(discovered["supportedVersions"][0], "2026-07-28");
    assert_eq!(discovered["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "tondo");
    let tools = agent.request("tools/list", json!({}));
    assert!(tools["ttlMs"].as_u64().is_some() && tools["cacheScope"] == "public");
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for name in ["check", "fix", "run", "test", "rename", "counters", "dump_read", "debug_start", "debug"] {
        assert!(names.contains(&name), "{name}");
    }
    let unsupported =
        agent.send("tools/list", json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "1900-01-01" } }));
    assert_eq!(unsupported["error"]["code"], -32022);
    assert_eq!(unsupported["error"]["data"]["requested"], "1900-01-01");
    let skill = agent.request("resources/read", json!({ "uri": "tondo://skill" }));
    assert!(skill["contents"][0]["text"].as_str().unwrap().starts_with("---\nname: tondo"));

    // Legacy: the handshake, then requests without a version.
    let initialized = agent.send(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": { "name": "test", "version": "1" } }),
    );
    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(initialized["result"]["serverInfo"]["name"], "tondo");
    assert_eq!(agent.send("ping", json!({}))["result"], json!({}));
    let guide = agent.send("tools/call", json!({ "name": "guide", "arguments": { "list": true } }));
    assert!(guide["result"]["content"][0]["text"].as_str().unwrap().contains("pitfalls"));
    assert!(guide["result"].get("resultType").is_none());
    let missing = agent.send("resources/read", json!({ "uri": "tondo://nothing" }));
    assert_eq!(missing["error"]["code"], -32002);
    agent.stop();
}

const BROKEN: &str = "import std.console

fn greet(name: String): String {
    return \"hello {name}\"
}

fn main() {
    let ready = true && true;
    console.println(greet(\"ada\"))
}
";

#[test]
fn checks_fixes_and_navigates() {
    let dir = project("mcp_code", &[("main.to", BROKEN)]);
    let mut agent = Agent::start(&dir);
    let (checked, text) = agent.call("check", json!({ "path": "main.to" }));
    assert_eq!(checked["compiles"], false);
    let diagnostics = checked["diagnostics"].as_array().unwrap();
    assert_eq!(
        diagnostics[0]["at"],
        json!({ "file": "main.to", "line": 8, "column": 22, "end_line": 8, "end_column": 24 })
    );
    assert_eq!(diagnostics[0]["fixes"][0]["safe"], true);
    assert!(text.contains("write `and` (safe)"), "{text}");

    let (fixed, _) = agent.call("fix", json!({ "path": "main.to" }));
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 2);
    assert_eq!(fixed["compiles"], true, "{fixed}");
    let source = std::fs::read_to_string(dir.join("main.to")).unwrap();
    assert!(source.contains("let ready = true and true\n"), "{source}");

    let at = json!({ "file": "main.to", "line": 9, "column": 21 });
    let (definition, _) = agent.call("definition", at.clone());
    assert_eq!(definition["places"][0]["line"], 3);
    assert_eq!(definition["places"][0]["column"], 4);
    let (references, _) = agent.call("references", at.clone());
    assert_eq!(references["places"].as_array().unwrap().len(), 2);
    let (hover, _) = agent.call("hover", at.clone());
    assert!(hover["text"].as_str().unwrap().contains("fn greet(name: String): String"));
    let mut renamed = at;
    renamed["new_name"] = json!("welcome");
    agent.call("rename", renamed);
    let source = std::fs::read_to_string(dir.join("main.to")).unwrap();
    assert!(source.contains("fn welcome(") && source.contains("console.println(welcome(\"ada\"))"), "{source}");
    let (symbols, _) = agent.call("symbols", json!({ "file": "main.to" }));
    assert_eq!(symbols["symbols"][0]["name"], "welcome");
    // Who calls it, and what a code means.
    let (calls, text) = agent.call("calls", json!({ "file": "main.to", "line": 3, "column": 4 }));
    assert_eq!(calls["calls"][0]["function"], "main", "{text}");
    let (explained, text) = agent.call("explain", json!({ "code": "E1001" }));
    assert_eq!(explained["name"], "unknown-name");
    assert!(text.starts_with("E1001 unknown-name (error)"), "{text}");

    // A project made, and a dependency added to it and taken out.
    let (made, _) = agent.call("new_project", json!({ "path": "made" }));
    assert_eq!(made["name"], "made");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    agent.call("new_project", json!({ "path": "lib", "init": true }));
    let (added, _) = agent.call("add_dependency", json!({ "alias": "lib", "path": "../lib", "project": "made" }));
    assert_eq!(added["report"]["format"], "tondo-add-0.1/1", "{added}");
    assert!(std::fs::read_to_string(dir.join("made/tondo.toml")).unwrap().contains("lib = { path = \"../lib\" }"));
    agent.call("remove_dependency", json!({ "alias": "lib", "project": "made" }));
    assert!(!std::fs::read_to_string(dir.join("made/tondo.toml")).unwrap().contains("lib ="));
    agent.stop();
}

const ECHO: &str = "import std.console

fn twice(n: Int): Int {
    return n * 2
}

fn main() {
    let line = console.readLine().unwrapOr(\"\")
    console.println(\"{line} {twice(21)}\")
}
";

const TESTS: &str = "test doubles {
    assert(twice(2) == 4)
}

test fails {
    assert(twice(2) == 5, \"wrong\")
}
";

#[test]
fn runs_tests_and_debugs() {
    let dir = project("mcp_run", &[("src/main.to", ECHO), ("src/main_test.to", TESTS)]);
    let mut agent = Agent::start(&dir);
    let (ran, _) = agent.call("run", json!({ "stdin": "read\n" }));
    assert_eq!(ran["stdout"], "read 42\n", "{ran}");
    assert_eq!(ran["exit_code"], 0);

    let (tested, text) = agent.call("test", json!({ "coverage": true }));
    assert_eq!(tested["coverage"]["format"], "tondo-coverage-0.1/1", "{tested}");
    assert!(text.contains("coverage:\n  src/main.to"), "{text}");
    assert_eq!(tested["report"]["summary"]["passed"], 1);
    assert_eq!(tested["report"]["summary"]["failed"], 1);
    assert!(text.contains("assertion-failed: wrong"), "{text}");

    // What the program is made of, and a build, as data.
    let (graph, text) = agent.call("graph", json!({ "path": "src/main.to" }));
    assert_eq!(graph["modules"][0]["imports"][0]["module"], "std.console", "{graph}");
    assert!(text.contains("  -> std.console\n"), "{text}");
    let (built, _) = agent.call("build", json!({ "output": "built" }));
    assert_eq!(
        (&built["report"]["format"], &built["report"]["output"]),
        (&"tondo-build-0.1/1".into(), &"built".into())
    );
    assert!(dir.join("built").is_file());

    let (started, _) = agent.call("debug_start", json!({ "target": "." }));
    let session = started["session"].clone();
    assert_eq!(started["said"].as_array().unwrap().last().unwrap()["stopped"]["why"], "start");
    let debug =
        |agent: &mut Agent, command: &str| agent.call("debug", json!({ "session": session, "command": command })).0;
    assert!(debug(&mut agent, "break src/main.to:4")["said"][0]["breakpoint"].is_object());
    let stopped = debug(&mut agent, "continue");
    assert_eq!(stopped["said"][1]["stopped"]["at"], "src/main.to:4 in twice");
    let locals = debug(&mut agent, "locals");
    assert_eq!(locals["said"][0]["locals"][0], json!({ "name": "n", "type": "Int", "value": "21", "children": 0 }));
    debug(&mut agent, "delete 1");
    let ended = debug(&mut agent, "continue");
    assert_eq!(ended["ended"], true);
    assert_eq!(ended["output"], " 42\n");

    // A benchmark, measured.
    let bench = "import std.testing\n\ntest doubling {\n    testing.bench(\"twice\", () { twice(21) })\n}\n";
    std::fs::write(dir.join("src/bench_test.to"), bench).unwrap();
    let (measured, text) = agent.call("test", json!({ "bench": true, "bench_time": 0.1 }));
    let benchmark = &measured["report"]["tests"][0]["attempts"][0]["benchmarks"][0];
    assert_eq!(benchmark["name"], "twice", "{measured}");
    assert!(benchmark["ns_per_op"]["median"].as_f64().unwrap() > 0.0, "{benchmark}");
    assert!(text.contains("  twice "), "{text}");

    // A search for an input that fails, kept and reduced (testing 10.6).
    let fuzz = "import std.testing\n\ntest parsesCodes {\n    testing.fuzz(\"code\", [1], (n: Int) {\n        assert(n != 4242)\n    })\n}\n";
    std::fs::write(dir.join("src/fuzz_test.to"), fuzz).unwrap();
    let (searched, text) = agent.call("test", json!({ "fuzz": 60 }));
    let call = &searched["report"]["tests"][0]["attempts"][0]["fuzz"][0];
    assert_eq!(call["input"]["json"], "4242", "{searched}");
    assert!(call["input"]["file"].as_str().unwrap().starts_with("fuzz/unit/mcp_run/parsesCodes/code/"), "{call}");
    assert!(text.contains("reproduce: tondo test --exact mcp_run::unit::mcp_run::parsesCodes"), "{text}");
    agent.stop();
}

const PROFILED: &str = "import std.console

fn fib(n: Int): Int {
    if n < 2 {
        return n
    }
    fib(n - 1) + fib(n - 2)
}

fn main() {
    console.println(\"{fib(35)}\")
}
";

const PROFILED_TESTS: &str = "import std.testing

test slow {
    assert(fib(35) == 9227465)
}

test joining {
    testing.bench(\"join\", () { \"{[1, 2, 3]}\" })
}
";

/// An agent finds where the time goes: in the program, in a test, and in
/// what a benchmark measures.
#[test]
fn profiles_for_agents() {
    let files = [
        ("tondo.toml", "[package]\nname = \"app\"\n"),
        ("src/main.to", PROFILED),
        ("src/main_test.to", PROFILED_TESTS),
    ];
    let dir = project("mcp_profile", &files);
    let mut agent = Agent::start(&dir);
    let (profiled, text) = agent.call("profile", json!({}));
    let profile = &profiled["profiles"][0];
    assert_eq!((&profile["type"], &profile["functions"][0]["function"]), (&json!("cpu"), &json!("fib")), "{profiled}");
    // The line that recurses is in every stack, with its code.
    let line = profile["lines"].as_array().unwrap().iter().find(|l| l["line"] == 7).expect("fib's call");
    assert_eq!((&line["file"], &line["source"]), (&json!("src/main.to"), &json!("fib(n - 1) + fib(n - 2)")));
    assert!(text.contains("src/main.to:7 in fib  fib(n - 1) + fib(n - 2)"), "{text}");
    let (profiled, _) = agent.call("profile", json!({ "test": "app::unit::app::slow" }));
    assert_eq!(profiled["profiles"][0]["functions"][0]["function"], "fib", "{profiled}");
    let (profiled, _) = agent.call("profile", json!({ "bench": true, "kind": "memory" }));
    let profile = &profiled["profiles"][0];
    assert_eq!(profile["type"], "alloc_space", "{profiled}");
    assert!(profile["total"].as_i64().unwrap() > 0, "{profiled}");
    agent.stop();
}

/// `tondo setup` writes the skill for Claude Code and Codex, registers
/// the server with their commands, and installs the extension in the
/// editors, here with commands that say how they were run; `gh` gives an
/// extension of CI. `PATH` has only them, so no real one runs.
#[test]
fn setup_readies_the_agents() {
    let dir = project("mcp_setup", &[]);
    let (home, commands) = (dir.join("home"), dir.join("commands"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&commands).unwrap();
    let editors = ["code", "code-insiders", "cursor", "codium", "windsurf"];
    for command in ["claude", "codex", "gh"].iter().chain(&editors) {
        let mut script = format!("#!/bin/sh\necho \"{command} $*\" >> {}\n", dir.join("calls").display());
        if *command == "gh" {
            script += "case $2 in\nlist) echo 42 ;;\ndownload) for a; do [ \"$p\" = -D ] && : > \"$a/tondo-0.1.0.vsix\"; p=$a; done ;;\nesac\n";
        }
        std::fs::write(commands.join(command), script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(commands.join(command), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = commands.clone();
    let setup = || {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .arg("setup")
            .env("HOME", &home)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let said = setup();
    assert!(said.contains("it compiles and runs a program, and its MCP server answers"), "{said}");
    assert!(said.contains("start a new session of Claude Code and Codex"), "{said}");
    for skill in [".claude/skills/tondo/SKILL.md", ".agents/skills/tondo/SKILL.md"] {
        assert_eq!(std::fs::read_to_string(home.join(skill)).unwrap(), tondo::guide::skill());
    }
    let tondo = Path::new(env!("CARGO_BIN_EXE_tondo")).canonicalize().unwrap();
    let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
    assert!(calls.contains(&format!("claude mcp add --scope user tondo -- {} mcp\n", tondo.display())), "{calls}");
    assert!(calls.contains(&format!("codex mcp add tondo -- {} mcp\n", tondo.display())), "{calls}");
    assert!(calls.contains("gh run download 42 -R tonyredondo/tondo -n tondo-vscode -D "), "{calls}");
    for editor in editors {
        let installed = calls.lines().find(|line| line.starts_with(&format!("{editor} --install-extension "))).unwrap();
        assert!(installed.ends_with("/tondo-0.1.0.vsix --force"), "{calls}");
    }
    assert!(said.contains("Cursor: the Tondo extension is installed; reload its windows"), "{said}");
    // Again, it replaces what it registered; as data, it says what it did.
    // A .vsix given is the one installed.
    let vsix = dir.join("tondo.vsix");
    std::fs::write(&vsix, "").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["setup", "--json", "--vsix"])
        .arg(&vsix)
        .env("HOME", &home)
        .env("PATH", &path)
        .output()
        .unwrap();
    let said: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (&said["format"], &said["ok"], &said["tondo"]),
        (&"tondo-setup-0.1/1".into(), &true.into(), &json!(tondo))
    );
    let agents: Vec<(&str, bool)> =
        said["agents"].as_array().unwrap().iter().map(|a| (a["name"].as_str().unwrap(), a["mcp"] == true)).collect();
    assert_eq!(agents, [("Claude Code", true), ("Codex", true)]);
    assert_eq!(said["agents"][0]["skill"], json!(home.join(".claude/skills/tondo/SKILL.md")));
    assert!(said["messages"][0].as_str().unwrap().contains("its MCP server answers"), "{said}");
    let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
    assert_eq!(calls.matches("claude mcp remove --scope user tondo\n").count(), 2, "{calls}");
    assert!(calls.contains(&format!("windsurf --install-extension {} --force\n", vsix.display())), "{calls}");
    let readied: Vec<(&str, bool)> = said["editors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["extension"] == true))
        .collect();
    assert_eq!(
        readied,
        [("VS Code", true), ("VS Code Insiders", true), ("Cursor", true), ("VSCodium", true), ("Windsurf", true)]
    );
}

const LIVE: &str = "import std.console

fn answer(line: String): String {
    \"v1 {line}\"
}

for {
    match console.readLine() {
        some(line) => console.println(answer(line))
        none => break
    }
}
";

#[test]
fn live_sessions_reload() {
    let dir = project("mcp_live", &[("server.to", LIVE)]);
    let mut agent = Agent::start(&dir);
    let (started, _) = agent.call("run_start", json!({ "path": "server.to" }));
    assert_eq!(started["running"], true, "{started}");
    let session = started["session"].clone();
    let (heard, _) = agent.call("run_input", json!({ "session": session, "text": "a\n", "wait": 30 }));
    assert_eq!(heard["stdout"], "v1 a\n", "{heard}");
    let source = std::fs::read_to_string(dir.join("server.to")).unwrap().replace("\"v1 {line}\"", "\"v2 {line}\"");
    std::fs::write(dir.join("server.to"), source).unwrap();
    let (reloaded, _) = agent.call("reload", json!({ "session": session }));
    assert_eq!(reloaded["changed"], json!(["answer"]));
    let (heard, _) = agent.call("run_input", json!({ "session": session, "text": "b\n", "wait": 30 }));
    assert_eq!(heard["stdout"], "v2 b\n", "{heard}");
    let (stopped, _) = agent.call("run_stop", json!({ "session": session }));
    assert_eq!(stopped["running"], false);
    agent.stop();
}
