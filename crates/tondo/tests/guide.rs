//! Every example of `tondo guide` compiles, as a library for C when its
//! info string says `library`, and every example of a mistake fails with
//! the error it names.

use std::path::Path;
use std::process::Command;

/// The examples of a topic: their info string and their code.
fn examples(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.strip_prefix("```") else { continue };
        let code: Vec<&str> = lines.by_ref().take_while(|l| !l.starts_with("```")).collect();
        if info.starts_with("tondo") {
            out.push((info.to_string(), code.join("\n") + "\n"));
        }
    }
    out
}

#[test]
fn examples_compile() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("guide");
    let _ = std::fs::remove_dir_all(&dir);
    let mut jobs = Vec::new();
    let readme = ("readme", include_str!("../../../README.md"));
    for (topic, text) in tondo::guide::TOPICS.iter().copied().chain([readme]) {
        for (index, (info, code)) in examples(text).into_iter().enumerate() {
            let case = dir.join(format!("{topic}-{index}"));
            std::fs::create_dir_all(&case).unwrap();
            // A runbook's server takes its port when it runs; tests go in a
            // project's file of tests.
            match info == "tondo test" {
                true => {
                    std::fs::create_dir_all(case.join("src")).unwrap();
                    std::fs::write(case.join("tondo.toml"), "[package]\nname = \"guide\"\n").unwrap();
                    std::fs::write(case.join("src/main.to"), "fn main() {}\n").unwrap();
                    std::fs::write(case.join("src/main_test.to"), &code).unwrap();
                }
                false => std::fs::write(case.join("main.to"), code.replace("<port>", "8080")).unwrap(),
            }
            jobs.push((format!("{topic} example {index} ({info})"), info, case, code));
        }
    }
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(8)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for (name, info, case, code) in chunk {
                        // A test needs a project, whose tests `--list` builds.
                        let checks: &[&str] =
                            if info == "tondo test" { &["test", "--list"] } else { &["check", "main.to"] };
                        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                            .args(checks)
                            .args(info.ends_with(" library").then_some("--library"))
                            .current_dir(case)
                            .output()
                            .unwrap();
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let expected = info.strip_prefix("tondo error ").and_then(|rest| rest.split(' ').next());
                        let ok = match expected {
                            None => output.status.success(),
                            Some(code) => !output.status.success() && stderr.contains(&format!("error[{code}]")),
                        };
                        if !ok {
                            failures.push(format!("{name}:\n{code}\n{stderr}"));
                        }
                    }
                    failures
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `tondo guide cli` names every command, and `tondo guide mcp` every tool
/// of `tondo mcp`, so that neither falls behind the toolchain.
#[test]
fn the_cli_and_mcp_topics_cover_everything() {
    let cli = tondo::guide::topic("cli").unwrap();
    let commands: Vec<&str> = tondo::cli::COMMANDS.iter().map(|command| command.name).collect();
    let missing: Vec<&&str> = commands.iter().filter(|name| !cli.contains(&format!("tondo {name}"))).collect();
    assert!(missing.is_empty(), "`tondo guide cli` does not name: {missing:?}");
    let mcp = tondo::guide::topic("mcp").unwrap();
    let missing: Vec<&str> = tondo::mcp::tool_names().filter(|name| !mcp.contains(&format!("`{name}`"))).collect();
    assert!(missing.is_empty(), "`tondo guide mcp` does not name: {missing:?}");
}

/// The repository carries the skill for the agents that work in it, where
/// Claude Code and the Agent Skills standard look; it stays the guide's.
#[test]
fn the_repository_has_the_skill() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in [".agents/skills/tondo/SKILL.md", ".claude/skills/tondo/SKILL.md"] {
        let carried = std::fs::read_to_string(root.join(path)).unwrap_or_default();
        assert!(
            carried == tondo::guide::skill(),
            "{path} is not the guide's skill: write it again with `cargo run -q -p tondo -- guide --skill > .agents/skills/tondo/SKILL.md`"
        );
    }
}

/// Every code the specification gives is in `tondo explain` and in a test, so
/// that the specification and the compiler do not part in silence.
#[test]
fn every_code_is_explained_and_tested() {
    let specs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec");
    let mut codes = Vec::new();
    for spec in std::fs::read_dir(specs).unwrap().flatten() {
        for line in std::fs::read_to_string(spec.path()).unwrap().lines() {
            let code = line.strip_prefix("| `").and_then(|rest| rest.split_once("` |")).map(|(code, _)| code);
            let is_code = |code: &&str| code.len() == 5 && code[1..].bytes().all(|byte| byte.is_ascii_digit());
            codes.extend(code.filter(is_code).filter(|code| code.starts_with(['E', 'W', 'P'])).map(String::from));
        }
    }
    assert!(codes.len() > 100);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut tested = String::new();
    let mut dirs = vec![root.join("tests"), root.join("crates/tondo/tests"), root.join("crates/tondo/guide")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            match entry.path().is_dir() {
                true => dirs.push(entry.path()),
                false => tested.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default()),
            }
        }
    }
    let explained = include_str!("../src/explain.rs");
    for code in &codes {
        assert!(explained.contains(&format!("\"{code}\"")), "`tondo explain` does not know {code}");
        assert!(tested.contains(code.as_str()), "no test says {code}: add a case");
    }
}

/// An alias holds across the modules a program brings, so that one of the
/// standard library names one module: a test of a program that imports both
/// would not compile otherwise.
#[test]
fn an_alias_of_the_standard_library_names_one_module() {
    let mut aliases = std::collections::HashMap::new();
    for (file, text) in tondo::stdlib::sources() {
        for line in text.lines() {
            let Some((module, alias)) = line.strip_prefix("import ").and_then(|rest| rest.split_once(" as ")) else {
                continue;
            };
            let held = aliases.entry(alias.to_string()).or_insert_with(|| module.to_string());
            assert_eq!(held, module, "`{alias}` names two modules: std/{file}.to has the second");
        }
    }
}

/// Every command of the toolchain, its tool and how the editor has it, and
/// every option, in the tools: what is added to the command line without
/// them fails here. `tondo.runCommand` runs the options the editor has no
/// place of its own for.
#[test]
fn the_command_line_is_in_the_tools_and_the_editor() {
    const MCP: &str = include_str!("../src/mcp.rs");
    const EDITOR: &str = include_str!("../../../editors/vscode/package.json");
    // What serves or joins the others, with no use from inside them.
    const NONE: &str = "none";
    let reached = [
        ("run", "run", "tondo.run"),
        ("build", "build", "tondo.buildLibrary"),
        ("check", "check", "tondo.unreached"),
        ("fix", "fix", "tondo.runCommand"),
        ("expand", "expand", "tondo.expand"),
        ("fmt", "format", "tondo.compact"),
        ("api", "api", "tondo.showApi"),
        ("graph", "graph", "tondo.exportGraph"),
        ("optimizations", "optimizations", "tondo.showOptimizations"),
        ("guide", "guide", "tondo.guide"),
        ("explain", "explain", "tondo.runCommand"),
        ("model", "model", "tondo.modelTests"),
        ("update", "update", "tondo.updateDependencies"),
        ("new", "new_project", "tondo.newProject"),
        ("init", "new_project", "tondo.initProject"),
        ("add", "add_dependency", "tondo.addDependency"),
        ("remove", "remove_dependency", "tondo.removeDependency"),
        ("clean", "clean", "tondo.clean"),
        ("test", "test", "tondo.affectedTests"),
        ("doc-test", "doc_test", "tondo.runCommand"),
        ("setup", "setup", "tondo.runCommand"),
        ("ps", "ps", "tondo.pickProcess"),
        ("source", "source", "tondo.runCommand"),
        ("logpoint", "logpoint", "tondo.logpoint"),
        ("blocked", "blocked", "tondo.blocked"),
        ("requests", "requests", "tondo.refreshRequests"),
        ("counters", "counters", "tondo.watchCounters"),
        ("debug", "debug_start", "tondo.debugLive"),
        ("values", "values", "tondo.liveValues"),
        ("eval", "eval", "tondo.eval"),
        ("reload", "reload", "tondo.reload"),
        ("dump", "dump", "tondo.dump"),
        ("trace", "trace", "tondo.trace"),
        ("profile", "profile", "tondo.profile"),
        ("target", "target_pack", "tondo.targetPack"),
        ("bind", "bind", "tondo.bind"),
        ("version", "version", "tondo.runCommand"),
        ("lsp", NONE, NONE),
        ("mcp", NONE, NONE),
        ("dap", NONE, NONE),
        ("relay", NONE, NONE),
        ("completions", NONE, NONE),
        ("help", NONE, NONE),
    ];
    for command in tondo::cli::COMMANDS {
        let Some((_, tool, editor)) = reached.iter().find(|(name, ..)| *name == command.name) else {
            panic!("`tondo {}` is in neither the tools nor the editor: add it to both, and here", command.name);
        };
        assert!(*tool == NONE || MCP.contains(&format!("name: \"{tool}\"")), "`{}` has no tool `{tool}`", command.name);
        assert!(
            *editor == NONE || EDITOR.contains(&format!("\"command\": \"{editor}\"")),
            "`{}`: no `{editor}`",
            command.name
        );
    }
    assert!(EDITOR.contains("\"command\": \"tondo.runCommand\""), "the options the editor has no place for");
    // Options, by the name of the parameter of the tools; those of `tondo
    // test` go in `flags`, and the tools always speak data.
    let spoken = ["--json", "--diagnostic-format", "--test-format", "--help"];
    let tools = [("--compare", "debug_compare"), ("--port", "dump_serve")];
    for command in tondo::cli::COMMANDS.iter().filter(|command| !matches!(command.name, "lsp" | "mcp" | "completions"))
    {
        for flag in command.usage.iter().flat_map(|usage| usage.split(|c: char| !(c.is_alphanumeric() || c == '-'))) {
            let Some(name) = flag.strip_prefix("--").filter(|_| !spoken.contains(&flag)) else { continue };
            let parameter = format!("\"{}\"", name.replace('-', "_"));
            let known = MCP.contains(&format!("\"{flag}\""))
                || MCP.contains(&parameter)
                || command.name == "test"
                || tools.iter().any(|(option, tool)| *option == flag && MCP.contains(&format!("name: \"{tool}\"")));
            assert!(known, "`tondo {} {flag}` has no parameter in the tools", command.name);
        }
    }
}

#[test]
fn what_observes_is_in_the_tools_and_the_editor() {
    const CLI: &str = include_str!("../src/cli.rs");
    const MCP: &str = include_str!("../src/mcp.rs");
    const EDITOR: &str = include_str!("../../../editors/vscode/package.json");
    const DEBUGGER: &str = "debug adapter";
    // Each command, its tool and how the editor has it: its command, or
    // what stands for it.
    let reached = [
        ("tondo ps", "ps", "tondo.pickProcess"),
        ("tondo counters", "counters", "tondo.watchCounters"),
        ("tondo trace", "trace", "tondo.trace"),
        ("tondo blocked", "blocked", "tondo.blocked"),
        ("tondo logpoint", "logpoint", "tondo.logpoint"),
        ("tondo requests", "requests", "tondo.refreshRequests"),
        ("tondo eval", "eval", "tondo.eval"),
        ("tondo values", "values", "tondo.liveValues"),
        ("tondo source", "source", DEBUGGER),
        ("tondo reload", "reload", "tondo.reload"),
        ("tondo profile", "profile", "tondo.profile"),
        ("tondo dump <pid", "dump", "tondo.dump"),
        ("tondo dump read", "dump_read", "tondo.debugDump"),
        ("tondo dump diff", "dump_diff", "tondo.compareDumps"),
        ("tondo dump group", "dump_group", "tondo.showFailures"),
        ("tondo dump issue", "dump_issue", "tondo.failureIssue"),
        ("tondo dump redact", "dump_redact", "tondo.redactDump"),
        ("tondo dump report", "dump_report", "tondo.dumpReport"),
        ("tondo dump trend", "dump_trend", "tondo.dumpTrend"),
        ("tondo dump test", "dump_test", "tondo.testFromDump"),
        ("tondo dump serve", "dump_serve", "tondo.serveDumps"),
        ("tondo dump core", "dump_core", "tondo.readCore"),
    ];
    for (usage, tool, editor) in reached {
        assert!(CLI.contains(usage), "the command `{usage}` is gone: take it from this list");
        assert!(MCP.contains(&format!("name: \"{tool}\"")), "`{usage}` has no tool `{tool}`");
        assert!(
            editor == DEBUGGER || EDITOR.contains(&format!("\"command\": \"{editor}\"")),
            "`{usage}`: no `{editor}`"
        );
    }
    for name in CLI.split("\"tondo dump ").skip(1).map(|rest| rest.split([' ', '"']).next().unwrap_or_default()) {
        let known =
            name.starts_with('<') || name == "plugin" || reached.iter().any(|(usage, ..)| usage.ends_with(name));
        assert!(known, "`tondo dump {name}` is in neither the tools nor the editor: add it here too");
    }
    // What other commands take for this, with the tool's name of it.
    for (flag, tool, editor) in [
        ("api --diff", "\"api_diff\"", "tondo.apiDiff"),
        ("--stop <seconds>", "\"stop\"", "tondo.pickProcess"),
        ("--affected", "\"affected\"", "tondo.affectedTests"),
        ("--bisect", "\"test_bisect\"", "tondo.bisectTest"),
        ("--lock-order", "\"lock_order\"", "tondo.lockOrderTests"),
    ] {
        assert!(CLI.contains(flag), "`{flag}` is gone: take it from this list");
        assert!(MCP.contains(tool), "`{flag}` has no tool");
        assert!(EDITOR.contains(&format!("\"command\": \"{editor}\"")), "`{flag}`: no `{editor}`");
    }
}
