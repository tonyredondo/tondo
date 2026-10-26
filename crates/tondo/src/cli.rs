//! The commands of `tondo`, as one table: what `tondo help` shows, what
//! `tondo <command> --help` shows of one, and what a shell completes
//! (`tondo completions`). Each command's flags are those its usage names.

/// A command: its name, the lines of its usage, and what it does.
pub struct Command {
    pub name: &'static str,
    pub usage: &'static [&'static str],
    pub summary: &'static [&'static str],
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "run",
        usage: &[
            "tondo run [<project>|<file.to>] [--link <library>]... [--static] [--debug | --live | --watch]",
            "          [--target <target>] [--diagnostic-format human|json] [-- <argument>...]",
        ],
        summary: &[
            "compile and run a program (under qemu for another",
            "machine; live: its code changes as it runs, with",
            "`tondo reload`, or by itself as its files change)",
        ],
    },
    Command {
        name: "build",
        usage: &[
            "tondo build [<project>|<file.to>] [--output <path>] [--link <library>]... [--static] [--strip]",
            "          [--debug | --live] [--target <target>] [--diagnostic-format human|json] [--json]",
        ],
        summary: &[
            "compile a native executable (static on Linux;",
            "without its symbol table; static for a target:",
            "x86_64-linux or aarch64-linux)",
        ],
    },
    Command {
        name: "build",
        usage: &["tondo build --library [<project>|<file.to>] [--output <dir>] [--debug] [--json]"],
        summary: &["compile a library for C, lib<name>.so (.dylib)", "and lib<name>.a, with its header <name>.h"],
    },
    Command {
        name: "check",
        usage: &["tondo check [<project>|<file.to>] [--library] [--json]"],
        summary: &["report diagnostics only (of a library; as JSON)"],
    },
    Command {
        name: "expand",
        usage: &["tondo expand [<project>|<file.to>] [--json]"],
        summary: &["show what each `derive` expanded to"],
    },
    Command {
        name: "fmt",
        usage: &["tondo fmt [--check | --compact] [--json] <file.to>..."],
        summary: &["format files canonically, or print them compactly"],
    },
    Command {
        name: "api",
        usage: &["tondo api [<module>|<file.to>] [--private] [--list] [--project <dir>] [--json]"],
        summary: &["show a module's declarations and documentation"],
    },
    Command {
        name: "graph",
        usage: &[
            "tondo graph [<project>|<dir>|<file.to>] [--packages | --files] [--module <module> [--reverse]]",
            "          [--json | --dot | --mermaid]",
        ],
        summary: &["show what a program is made of: its packages,", "modules and files, and what each imports"],
    },
    Command {
        name: "guide",
        usage: &["tondo guide [<topic>] [--list] [--skill] [--json]"],
        summary: &["learn Tondo: the language, the library, the commands"],
    },
    Command {
        name: "update",
        usage: &["tondo update [<project>] [<dependency>...] [--json]"],
        summary: &["fetch dependencies again and rewrite tondo.lock"],
    },
    Command {
        name: "test",
        usage: &[
            "tondo test [<project>] [--filter <text> | --glob <pattern> | --exact <id>...] [--shard <i/n>] [--watch]",
            "          [--order canonical|random] [--schedules <n>] [--seed <hex>] [--list] [--jobs <n>] [--timeout <seconds>]",
            "          [--retry <n>] [--allow-flaky] [--repeat <n>] [--codeowners auto|none|<path>]",
            "          [--show-output] [--allow-empty] [--deny-skips] [--test-format human|json|events] [--json]",
            "          [--report json=<path> | junit=<path> | coverage=<path> | lcov=<path>]... [--coverage]",
            "          [--diagnostic-format human|json] [--artifacts <dir>] [--update-snapshots]",
            "          [--static] [--debug] [--library] [--target <target>]",
        ],
        summary: &["run the tests of a project (of a library); with --watch,", "again as its files change"],
    },
    Command {
        name: "doc-test",
        usage: &["tondo doc-test [--edition 0.1] <markdown>"],
        summary: &["check the examples of a document (as JSON)"],
    },
    Command {
        name: "lsp",
        usage: &["tondo lsp"],
        summary: &["serve an editor: diagnostics, hover, definitions,", "references, rename, completion, highlighting"],
    },
    Command {
        name: "mcp",
        usage: &["tondo mcp"],
        summary: &["serve an agent, with the Model Context Protocol:", "every command above as its tools"],
    },
    Command {
        name: "setup",
        usage: &["tondo setup [--json]"],
        summary: &["set up Claude Code and Codex: the skill, and tondo", "mcp for every project; checked"],
    },
    Command { name: "ps", usage: &["tondo ps [--json]"], summary: &["list the running Tondo programs"] },
    Command {
        name: "counters",
        usage: &["tondo counters <pid|name> [--interval <seconds>] [--count <n>] [--json]"],
        summary: &["watch a running program's counters, the runtime's", "and its own (std.diagnostics)"],
    },
    Command {
        name: "debug",
        usage: &["tondo debug [--json] [--live] <file.to|project|executable|pid|name> [-- <argument>...]"],
        summary: &[
            "debug a program built with --debug: breakpoints,",
            "steps, bindings, stacks, tasks; and with --live,",
            "`reload` to change its code where it stopped",
        ],
    },
    Command {
        name: "reload",
        usage: &["tondo reload <pid|name> [--json]"],
        summary: &["change a running live build's code to what its", "files say now"],
    },
    Command { name: "dap", usage: &["tondo dap"], summary: &["debug for an editor, with the Debug Adapter Protocol"] },
    Command {
        name: "dump",
        usage: &["tondo dump <pid|name> [--output <path>] [--json]"],
        summary: &["write a dump of a running program: its tasks and", "where they are, its counters, its live memory"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump read <file> [tasks | task <id> | heap | objects | object <address>] [--json]"],
        summary: &["show a dump, as one written at a crash with", "TONDO_DUMP=<dir>"],
    },
    Command {
        name: "trace",
        usage: &[
            "tondo trace <pid|name> [--duration <seconds>] [--output <path>] [--json]",
            "tondo trace [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]",
        ],
        summary: &["record what a program's tasks do, for Perfetto,", "and a profile of where it spends its time"],
    },
    Command {
        name: "target",
        usage: &["tondo target pack [<dir>] [--json]"],
        summary: &["write this machine's target, for a tondo elsewhere", "to build for it (TONDO_TARGETS=<dir>)"],
    },
    Command {
        name: "bind",
        usage: &[
            "tondo bind [<header.h>...] [--package <name>] [--library <library>]... [--project <dir>] [--output <file.to>]",
            "          [-I <dir>] [-D <name[=value]>] [--json]",
        ],
        summary: &["write a module of extern declarations for a C header"],
    },
    Command {
        name: "completions",
        usage: &["tondo completions bash|zsh|fish"],
        summary: &["what a shell completes of tondo's commands"],
    },
    Command {
        name: "help",
        usage: &["tondo help [<command>]"],
        summary: &["what tondo, or one of its commands, does"],
    },
    Command {
        name: "version",
        usage: &["tondo version [--json]"],
        summary: &["this tondo's version, target and backend"],
    },
];

/// What this `tondo` offers tools beyond its commands, as `tondo version
/// --json` lists it, for a client to check before it relies on one.
pub const FEATURES: &[&str] =
    &["json", "graph", "test-events", "test-exact-many", "test-watch", "test-coverage", "lsp-tests", "lsp-lenses"];

/// What a command writes with `--json`: one object, its format named for
/// the command (`tondo-<command>-0.1/1`), whether it succeeded, and what
/// the command gives.
pub fn print_json(command: &str, ok: bool, fields: serde_json::Value) {
    let mut object = serde_json::Map::new();
    object.insert("format".into(), format!("tondo-{command}-0.1/1").into());
    object.insert("ok".into(), ok.into());
    if let serde_json::Value::Object(fields) = fields {
        object.extend(fields);
    }
    println!("{}", serde_json::Value::Object(object));
}

/// Where a summary starts, after the usage.
const COLUMN: usize = 55;

const INTRODUCTION: &str = "New to Tondo, or a model writing it? Start with `tondo guide`.";

const NOTES: &str = "\
`tondo <file.to> [<argument>...]` runs a script (for #! lines). A project is
a directory with src/main.to, or with main.to or tondo.toml, given as
<project> or with --project <dir>; the current directory by default. --link
links a native library by name (-l) or file, besides a project's [native]
libraries. `tondo <command> --help` shows one command; with --json, a command
writes its result as one JSON object.";

/// What `tondo help` shows: every command.
pub fn usage() -> String {
    let mut text = format!("{INTRODUCTION}\n\nusage:\n");
    for command in COMMANDS {
        text.push_str(&entry(command));
    }
    text.push('\n');
    text.push_str(NOTES);
    text
}

/// What `tondo <command> --help` shows; `None` for no such command.
pub fn help(name: &str) -> Option<String> {
    let found: Vec<&Command> = COMMANDS.iter().filter(|c| c.name == name).collect();
    if found.is_empty() {
        return None;
    }
    let mut text = String::from("usage:\n");
    for command in found {
        text.push_str(&entry(command));
    }
    Some(text)
}

/// A command's usage, with its summary at `COLUMN`: beside the last line
/// when there is room, below it otherwise.
fn entry(command: &Command) -> String {
    let mut text = String::new();
    let (last, lines) = command.usage.split_last().expect("a usage");
    for line in lines {
        text.push_str(&format!("  {line}\n"));
    }
    let mut summary = command.summary.iter();
    match last.len() + 4 <= COLUMN {
        true => text.push_str(&format!("  {last:<width$}{}\n", summary.next().unwrap(), width = COLUMN - 2)),
        false => text.push_str(&format!("  {last}\n")),
    }
    for line in summary {
        text.push_str(&format!("{:COLUMN$}{line}\n", ""));
    }
    text
}

/// The commands' names, each once, in order.
pub fn names() -> Vec<&'static str> {
    let mut names: Vec<&str> = Vec::new();
    for command in COMMANDS {
        if !names.contains(&command.name) {
            names.push(command.name);
        }
    }
    names
}

/// The flags a command's usage names.
pub fn flags(name: &str) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    for line in COMMANDS.iter().filter(|c| c.name == name).flat_map(|c| c.usage) {
        for word in line.split(|c: char| c.is_whitespace() || "[]|".contains(c)) {
            let flag = word.trim_end_matches("...");
            let flag = flag.split('=').next().unwrap_or(flag);
            let is_flag = flag.len() > 1
                && flag.starts_with('-')
                && flag.chars().skip_while(|&c| c == '-').all(|c| c.is_ascii_alphanumeric() || c == '-');
            if is_flag && flag != "--" && !flags.iter().any(|f| f == flag) {
                flags.push(flag.to_string());
            }
        }
    }
    flags.push("--help".into());
    flags
}

/// What a shell needs to complete tondo's commands and their flags, and
/// files for the rest.
pub fn completions(shell: &str) -> Option<String> {
    let names = names();
    match shell {
        "bash" => {
            let mut cases = String::new();
            for name in &names {
                cases.push_str(&format!("        {name}) flags=\"{}\" ;;\n", flags(name).join(" ")));
            }
            Some(format!(
                "# tondo's completions for bash: `source <(tondo completions bash)`\n\
                 _tondo() {{\n\
                 \x20   local current=${{COMP_WORDS[COMP_CWORD]}}\n\
                 \x20   if [ \"$COMP_CWORD\" -eq 1 ]; then\n\
                 \x20       COMPREPLY=($(compgen -W \"{}\" -- \"$current\"))\n\
                 \x20       return\n\
                 \x20   fi\n\
                 \x20   local flags=\"\"\n\
                 \x20   case \"${{COMP_WORDS[1]}}\" in\n\
                 {cases}\
                 \x20   esac\n\
                 \x20   if [[ \"$current\" == -* ]]; then\n\
                 \x20       COMPREPLY=($(compgen -W \"$flags\" -- \"$current\"))\n\
                 \x20   fi\n\
                 }}\n\
                 complete -o default -F _tondo tondo\n",
                names.join(" ")
            ))
        }
        "zsh" => {
            let mut cases = String::new();
            for name in &names {
                cases.push_str(&format!("        {name}) compadd -- {} ;;\n", flags(name).join(" ")));
            }
            Some(format!(
                "#compdef tondo\n\
                 # tondo's completions for zsh: `source <(tondo completions zsh)`\n\
                 _tondo() {{\n\
                 \x20   if (( CURRENT == 2 )); then\n\
                 \x20       compadd -- {}\n\
                 \x20       return\n\
                 \x20   fi\n\
                 \x20   if [[ $PREFIX == -* ]]; then\n\
                 \x20       case $words[2] in\n\
                 {cases}\
                 \x20       esac\n\
                 \x20   else\n\
                 \x20       _files\n\
                 \x20   fi\n\
                 }}\n\
                 compdef _tondo tondo\n",
                names.join(" ")
            ))
        }
        "fish" => {
            let mut text = String::from(
                "# tondo's completions for fish: `tondo completions fish > ~/.config/fish/completions/tondo.fish`\n",
            );
            for command in COMMANDS {
                let summary = command.summary.join(" ").replace('\'', "\\'");
                text.push_str(&format!(
                    "complete -c tondo -n __fish_use_subcommand -a {} -d '{}'\n",
                    command.name, summary
                ));
            }
            for name in &names {
                for flag in flags(name) {
                    let (long, short) = match flag.strip_prefix("--") {
                        Some(long) => (format!("-l {long}"), String::new()),
                        None => (String::new(), format!("-s {}", flag.trim_start_matches('-'))),
                    };
                    text.push_str(&format!(
                        "complete -c tondo -n '__fish_seen_subcommand_from {name}' {long}{short}\n"
                    ));
                }
            }
            Some(text)
        }
        _ => None,
    }
}
