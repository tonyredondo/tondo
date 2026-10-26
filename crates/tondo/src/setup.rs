//! `tondo setup` (toolchain 7.9): readies the coding agents of this
//! machine for Tondo. It checks that this `tondo` compiles and runs a
//! program and that its MCP server answers, writes the skill where each
//! agent it finds reads skills, and registers `tondo mcp` with Claude Code
//! and Codex for every project, through their own commands. Running it
//! again updates all of it.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// An agent: its command, the directory its settings mark a machine with,
/// where it reads skills, and how its command adds and removes a server.
struct Agent {
    name: &'static str,
    command: &'static str,
    settings: &'static str,
    skills: &'static str,
    add: &'static [&'static str],
    remove: &'static [&'static str],
}

const AGENTS: [Agent; 2] = [
    Agent {
        name: "Claude Code",
        command: "claude",
        settings: ".claude",
        skills: ".claude/skills",
        add: &["mcp", "add", "--scope", "user", "tondo", "--"],
        remove: &["mcp", "remove", "--scope", "user", "tondo"],
    },
    // Codex reads the skills of the Agent Skills standard, as others do.
    Agent {
        name: "Codex",
        command: "codex",
        settings: ".codex",
        skills: ".agents/skills",
        add: &["mcp", "add", "tondo", "--"],
        remove: &["mcp", "remove", "tondo"],
    },
];

/// Readies the agents, saying what it did line by line, or, with `json`,
/// as `tondo-setup-0.1/1`: the agents readied and the same lines.
pub fn setup(json: bool) -> Result<(), String> {
    let tondo = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot find tondo itself: {e}"))?;
    check(&tondo)?;
    let mut said = Vec::new();
    let mut agents = Vec::new();
    macro_rules! say {
        ($($arg:tt)*) => {{
            let line = format!($($arg)*);
            if !json {
                std::println!("{line}");
            }
            said.push(line);
        }};
    }
    say!(
        "tondo {} at {}: it compiles and runs a program, and its MCP server answers",
        crate::driver::VERSION,
        tondo.display()
    );
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set: set it to your home directory")?);
    let mut found = Vec::new();
    for agent in &AGENTS {
        let command = on_path(agent.command);
        if command.is_none() && !home.join(agent.settings).is_dir() {
            continue;
        }
        found.push(agent.name);
        let skill = home.join(agent.skills).join("tondo/SKILL.md");
        std::fs::create_dir_all(skill.parent().expect("a directory"))
            .and_then(|()| std::fs::write(&skill, crate::guide::skill()))
            .map_err(|e| format!("cannot write {}: {e}", skill.display()))?;
        let Some(command) = command else {
            agents.push(serde_json::json!({ "name": agent.name, "skill": skill, "mcp": false }));
            say!(
                "{}: the skill is in {}; with `{}` installed, `tondo setup` registers the MCP server",
                agent.name,
                skill.display(),
                agent.command
            );
            continue;
        };
        let _ = Command::new(&command).args(agent.remove).stdout(Stdio::null()).stderr(Stdio::null()).status();
        let added = Command::new(&command)
            .args(agent.add)
            .arg(&tondo)
            .arg("mcp")
            .stdout(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run {}: {e}", command.display()))?;
        if !added.status.success() {
            return Err(format!(
                "`{} {} {} mcp` failed: {}",
                agent.command,
                agent.add.join(" "),
                tondo.display(),
                String::from_utf8_lossy(&added.stderr).trim()
            ));
        }
        agents.push(serde_json::json!({ "name": agent.name, "skill": skill, "mcp": true }));
        say!("{}: the skill is in {}, and tondo mcp serves every project", agent.name, skill.display());
    }
    let dir = tondo.parent().expect("a directory");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = std::env::split_paths(&path).any(|entry| entry.canonicalize().is_ok_and(|entry| entry == dir));
    if !on_path {
        say!(
            "{} is not on PATH: add it, as export PATH=\"{}:$PATH\" in your shell's profile",
            dir.display(),
            dir.display()
        );
    }
    match found.is_empty() {
        true => say!(
            "no Claude Code or Codex here: for another agent, `tondo guide --skill` prints the skill, and \
             `{} mcp` is the MCP server, over standard input and output",
            tondo.display()
        ),
        false => say!("start a new session of {} to load the skill and the MCP server", found.join(" and ")),
    }
    say!("new to Tondo? `tondo guide`");
    if json {
        crate::cli::print_json(
            "setup",
            true,
            serde_json::json!({
                "tondo": tondo,
                "version": crate::driver::VERSION,
                "agents": agents,
                "on_path": on_path,
                "messages": said,
            }),
        );
    }
    Ok(())
}

/// Checks that `tondo` compiles and runs a program, and that its MCP
/// server answers.
fn check(tondo: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("tondo-setup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
    let program = dir.join("hello.to");
    let ran =
        std::fs::write(&program, "import std.console\n\nfn main() {\n    console.println(\"hello from tondo\")\n}\n")
            .and_then(|()| Command::new(tondo).arg("run").arg(&program).output());
    let _ = std::fs::remove_dir_all(&dir);
    let ran = ran.map_err(|e| format!("cannot run a program: {e}"))?;
    if ran.stdout != b"hello from tondo\n" {
        return Err(format!(
            "tondo cannot run a program: {}{}",
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        ));
    }
    let mut server = Command::new(tondo)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start tondo mcp: {e}"))?;
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
    let mut stdin = server.stdin.take().expect("piped");
    let _ = writeln!(stdin, "{request}");
    drop(stdin);
    let mut answer = String::new();
    let _ = std::io::BufReader::new(server.stdout.take().expect("piped")).read_line(&mut answer);
    let _ = server.wait();
    match answer.contains("\"supportedVersions\"") {
        true => Ok(()),
        false => Err(format!("tondo mcp does not answer server/discover: {}", answer.trim())),
    }
}

/// Where a command is on `PATH`.
fn on_path(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(command)).find(|candidate| {
        use std::os::unix::fs::PermissionsExt;
        candidate.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}
