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
    for (topic, text) in tondo::guide::TOPICS {
        for (index, (info, code)) in examples(text).into_iter().enumerate() {
            let case = dir.join(format!("{topic}-{index}"));
            std::fs::create_dir_all(&case).unwrap();
            std::fs::write(case.join("main.to"), &code).unwrap();
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
                        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
                            .args(["check", "main.to"])
                            .args(info.ends_with(" library").then_some("--library"))
                            .current_dir(case)
                            .output()
                            .unwrap();
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let expected = info.strip_prefix("tondo error ").map(str::trim);
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

/// What looks at programs reaches an agent and an editor too: what the
/// terminal gets, both do, or the line here says how.
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
    // What `tondo test` takes for this, with the tool's name of it.
    for (flag, tool, editor) in [
        ("--affected", "\"affected\"", "tondo.affectedTests"),
        ("--bisect", "\"test_bisect\"", "tondo.bisectTest"),
        ("--lock-order", "\"lock_order\"", "tondo.lockOrderTests"),
    ] {
        assert!(MCP.contains(tool), "`{flag}` has no tool");
        assert!(EDITOR.contains(&format!("\"command\": \"{editor}\"")), "`{flag}`: no `{editor}`");
    }
}
