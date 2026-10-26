//! `tondo setup` (toolchain 7.9): readies the coding agents and the
//! editors of this machine for Tondo. It checks that this `tondo` compiles
//! and runs a program and that its MCP server answers, writes the skill
//! where each agent it finds reads skills, registers `tondo mcp` with
//! Claude Code and Codex for every project, through their own commands, and
//! installs the Tondo extension in each editor of the VS Code family it
//! finds. Running it again updates all of it.

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

/// An editor of the VS Code family: its command, and where that is in its
/// application on macOS, which does not put it on `PATH`.
struct Editor {
    name: &'static str,
    command: &'static str,
    app: &'static str,
}

const EDITORS: [Editor; 5] = [
    Editor { name: "VS Code", command: "code", app: "Visual Studio Code.app/Contents/Resources/app/bin/code" },
    Editor {
        name: "VS Code Insiders",
        command: "code-insiders",
        app: "Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders",
    },
    Editor { name: "Cursor", command: "cursor", app: "Cursor.app/Contents/Resources/app/bin/cursor" },
    Editor { name: "VSCodium", command: "codium", app: "VSCodium.app/Contents/Resources/app/bin/codium" },
    Editor { name: "Windsurf", command: "windsurf", app: "Windsurf.app/Contents/Resources/app/bin/windsurf" },
];

/// Where CI leaves the extension of each build of `main`.
const REPOSITORY: &str = "tonyredondo/tondo";

/// Readies the agents and the editors, saying what it did line by line,
/// or, with `json`, as `tondo-setup-0.1/1`: the agents and editors readied
/// and the same lines. The editors get the extension in `vsix`, or else
/// the one of the latest build of `main` that passed CI.
pub fn setup(vsix: Option<&Path>, json: bool) -> Result<(), String> {
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
    let editors: Vec<(&Editor, PathBuf)> =
        EDITORS.iter().filter_map(|editor| find_editor(editor, &home).map(|command| (editor, command))).collect();
    let mut readied = Vec::new();
    if !editors.is_empty() {
        let download = std::env::temp_dir().join(format!("tondo-setup-vsix-{}", std::process::id()));
        let extension = match vsix {
            Some(vsix) if vsix.is_file() => Ok(vsix.to_path_buf()),
            Some(vsix) => Err(format!("{} is not a file", vsix.display())),
            None => downloaded(&download),
        };
        for (editor, command) in editors {
            let installed = match &extension {
                Ok(vsix) => Command::new(&command)
                    .arg("--install-extension")
                    .arg(vsix)
                    .arg("--force")
                    .output()
                    .map_err(|e| format!("cannot run {}: {e}", command.display()))
                    .and_then(|out| match out.status.success() {
                        true => Ok(()),
                        false => Err(format!(
                            "`{} --install-extension` failed: {}",
                            editor.command,
                            String::from_utf8_lossy(&out.stderr).trim()
                        )),
                    }),
                Err(why) => Err(why.clone()),
            };
            readied.push(serde_json::json!({ "name": editor.name, "extension": installed.is_ok() }));
            match installed {
                Ok(()) => say!("{}: the Tondo extension is installed; reload its windows to start it", editor.name),
                Err(why) => say!("{}: the Tondo extension is not installed: {why}", editor.name),
            }
        }
        let _ = std::fs::remove_dir_all(&download);
    } else if vsix.is_some() {
        say!("no VS Code, Cursor, VSCodium or Windsurf here to install the extension in");
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
                "editors": readied,
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

/// An editor's command: on `PATH`, or on macOS in its application.
fn find_editor(editor: &Editor, home: &Path) -> Option<PathBuf> {
    on_path(editor.command).or_else(|| {
        let apps = [PathBuf::from("/Applications"), home.join("Applications")];
        apps.into_iter()
            .map(|apps| apps.join(editor.app))
            .find(|command| cfg!(target_os = "macos") && command.is_file())
    })
}

/// The extension of the latest build of `main` that passed CI, downloaded
/// into `dir` with GitHub's `gh`.
fn downloaded(dir: &Path) -> Result<PathBuf, String> {
    let how = "with GitHub's gh signed in, `tondo setup` downloads it; or `npm ci && npm run package` in \
               editors/vscode of Tondo's repository makes one, which `tondo setup --vsix <file>` installs";
    let gh = on_path("gh").ok_or(format!("gh is missing: {how}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
    let listed = Command::new(&gh)
        .args(["run", "list", "-R", REPOSITORY, "-b", "main", "-s", "success", "-w", "CI", "-L", "1"])
        .args(["--json", "databaseId", "-q", ".[0].databaseId"])
        .output()
        .map_err(|e| format!("cannot run gh: {e}"))?;
    let run = String::from_utf8_lossy(&listed.stdout).trim().to_string();
    if !listed.status.success() || run.is_empty() {
        return Err(format!("gh cannot list the builds of tondo: {how}"));
    }
    let fetched = Command::new(&gh)
        .args(["run", "download", &run, "-R", REPOSITORY, "-n", "tondo-vscode", "-D"])
        .arg(dir)
        .output()
        .map_err(|e| format!("cannot run gh: {e}"))?;
    let vsix = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|e| e == "vsix"));
    match vsix {
        Some(vsix) if fetched.status.success() => Ok(vsix),
        _ => Err(format!("gh cannot download it: {how}")),
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
