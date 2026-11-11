//! The commands of `tondo`, as one table: what `tondo help` shows, by
//! groups and then by command, what `tondo <command> --help` shows of one,
//! and what a shell completes (`tondo completions`). Each command's flags
//! are those its usage names.

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
            "tondo run [<project>|<file.to>] [--link <library>]... [--static] [--debug | --live | --watch [--tests]]",
            "          [--target <target>] [--diagnostic-format human|json] [-- <argument>...]",
            "tondo run --record <recording> [--last <duration> [--hold <duration>]] [<project>|<file.to>]",
            "          [--link <library>]...",
            "          [-- <argument>...]",
        ],
        summary: &[
            "compile and run a program (under qemu for another",
            "machine; live: its code changes as it runs, with",
            "`tondo reload`, or by itself as its files change;",
            "recorded, for `tondo debug --replay`)",
        ],
    },
    Command {
        name: "build",
        usage: &[
            "tondo build [<project>|<file.to>] [--output <path>] [--link <library>]... [--static] [--strip] [--no-source-links]",
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
        usage: &["tondo check [<project>|<file.to>] [--library] [--unreached] [--docs] [--performance] [--json]"],
        summary: &[
            "report diagnostics only (of a library; as JSON);",
            "--unreached, the code neither main nor a test reaches",
        ],
    },
    Command {
        name: "fix",
        usage: &["tondo fix [<project>|<file.to>] [--library] [--json]"],
        summary: &["apply the compiler's safe repairs to the program's files"],
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
        usage: &[
            "tondo api [<module>|<module>.<declaration>|<file.to>] [--private] [--list] [--project <dir>] [--json]",
            "tondo api --search <words> [--project <dir>] [--json]",
            "tondo api --diff <commit> [--project <dir>] [--json]",
        ],
        summary: &[
            "show a module's declarations, documentation and examples,",
            "or one type with its methods and example (std.http.Client,",
            "String); find declarations by their names and documentation",
        ],
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
        name: "model",
        usage: &["tondo model [<project>] <function> [--json]"],
        summary: &["write and run the test of a state machine of the program, with no model to write"],
    },
    Command {
        name: "explain",
        usage: &["tondo explain [<code>] [--json]"],
        summary: &["what the code of an error, a warning or a panic means, with examples"],
    },
    Command {
        name: "update",
        usage: &["tondo update [<project>] [<dependency>...] [--json]"],
        summary: &["fetch dependencies again and rewrite tondo.lock"],
    },
    Command {
        name: "new",
        usage: &["tondo new <name> [--json]"],
        summary: &["make a project in a new directory: tondo.toml,", "src/main.to and a test"],
    },
    Command {
        name: "init",
        usage: &["tondo init [<dir>] [--name <name>] [--json]"],
        summary: &["make a project of an existing directory"],
    },
    Command {
        name: "add",
        usage: &[
            "tondo add <alias> --path <dir> | --git <url> [--tag <tag> | --branch <branch> | --rev <commit>]",
            "          | --url <url> [--sha256 <hash>] [--dev] [--project <dir>] [--json]",
        ],
        summary: &["add a dependency to tondo.toml, and fetch it"],
    },
    Command {
        name: "remove",
        usage: &["tondo remove <alias> [--project <dir>] [--json]"],
        summary: &["take a dependency out of tondo.toml"],
    },
    Command {
        name: "clean",
        usage: &["tondo clean [--all] [--json]"],
        summary: &["empty the cache of builds; with --all, the fetched", "dependencies too"],
    },
    Command {
        name: "test",
        usage: &[
            "tondo test [<project>] [--filter <text> | --glob <pattern> | --exact <id>... | --affected <rev>]",
            "          [--bisect <commit> [--bisect-with <command>]] [--shard <i/n>] [--watch]",
            "          [--order canonical|random] [--schedules <n>|all|all:<n>|at:<choices>] [--seed <hex>] [--record <dir>] [--list] [--jobs <n>]",
            "          [--timeout <seconds>]",
            "          [--retry <n>] [--allow-flaky] [--lock-order] [--repeat <n>] [--codeowners auto|none|<path>]",
            "          [--show-output] [--allow-empty] [--deny-skips] [--test-format human|json|events] [--json]",
            "          [--report json=<path> | junit=<path> | coverage=<path> | lcov=<path> | markdown=<path>",
            "                    | html=<path>]... [--coverage]",
            "          [--bench [--bench-time <seconds>] [--save-baseline <name>] [--baseline <name> | --against <rev> | --compiler <tondo|rev>]",
            "                   [--max-regression <percent> [--regression-by time|instructions]] [--layouts <n>]]",
            "          [--profile cpu|memory]",
            "          [--fuzz [--fuzz-time <seconds>] [--fuzz-timeout <seconds>] [--fuzz-memory <MiB>] [--fuzz-corpus <dir>]]",
            "          [--models]",
            "          [--diagnostic-format human|json] [--artifacts <dir>] [--update-snapshots]",
            "          [--static] [--debug] [--library] [--target <target>] [--exec <command>]",
        ],
        summary: &[
            "run the tests of a project (of a library); with --watch,",
            "again as its files change; --bench measures its benchmarks,",
            "--fuzz searches for inputs that fail its fuzz tests, and",
            "--models checks the models of `std.model` alone; --exec runs",
            "the tests' executable under a command, as `perf record -g`",
        ],
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
        usage: &["tondo setup [--vsix <file>] [--json]"],
        summary: &[
            "set up Claude Code and Codex: the skill, and tondo",
            "mcp for every project; and the extension of VS Code",
            "and its kin; checked",
        ],
    },
    Command {
        name: "ps",
        usage: &["tondo ps [--via <command>] [--json]"],
        summary: &["list the running Tondo programs"],
    },
    Command {
        name: "source",
        usage: &["tondo source <file> --from <dump|recording|executable> [--line <n>] [--json]"],
        summary: &[
            "the file of a build as it was: here if it is the same,",
            "or from its repository, at the build's commit",
        ],
    },
    Command {
        name: "logpoint",
        usage: &[
            "tondo logpoint <pid|name> <file:line> [<text>] [--if <condition>] [--request <text>] [--dump <file>] [--stop <seconds>] [--max <n>] [--for <seconds>] [--json]",
        ],
        summary: &["what a line of a running program holds as it goes", "by, without stopping it, in any build"],
    },
    Command {
        name: "blocked",
        usage: &["tondo blocked <pid|name> [--json]"],
        summary: &["say who waits for whom in a running program"],
    },
    Command {
        name: "requests",
        usage: &["tondo requests <pid|name> [--json]"],
        summary: &["say what each request of a running program did, task by task"],
    },
    Command {
        name: "counters",
        usage: &["tondo counters <pid|name> [--interval <seconds>] [--count <n>] [--via <command>] [--json]"],
        summary: &["watch a running program's counters, the runtime's", "and its own (std.diagnostics)"],
    },
    Command {
        name: "debug",
        usage: &[
            "tondo debug [--json] [--live] <file.to|project|executable|pid|name> [-- <argument>...]",
            "tondo debug [--json] --test <id> [--schedule <seed>] [<project>]",
            "tondo debug [--json] --replay <recording>",
            "tondo debug [--json] --compare <recording> <recording>",
            "tondo debug [--json] --via <command> <pid|name>",
        ],
        summary: &[
            "debug a program built with --debug: breakpoints,",
            "steps, bindings, stacks, tasks; and with --live,",
            "`reload` to change its code where it stopped",
        ],
    },
    Command {
        name: "relay",
        usage: &["tondo relay <pid|name>"],
        summary: &["join standard input and output to a running", "program, for `debug --via \"ssh host\"`"],
    },
    Command {
        name: "values",
        usage: &["tondo values <pid|name> <file> [--for <seconds>] [--json]"],
        summary: &["what each line of a file left in a running live build,", "with nothing stopped"],
    },
    Command {
        name: "eval",
        usage: &["tondo eval <pid|name> <expression> [--every <seconds> [--count <n>]] [--json]"],
        summary: &["what an expression gives in a running live build,", "with its functions and types as they are now"],
    },
    Command {
        name: "reload",
        usage: &["tondo reload <pid|name> [--via <command>] [--json]"],
        summary: &["change a running live build's code to what its", "files say now"],
    },
    Command {
        name: "dap",
        usage: &["tondo dap [--link <library>]... [--static]"],
        summary: &["debug for an editor, with the Debug Adapter Protocol"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump <pid|name> [--output <path>] [--via <command>] [--json]"],
        summary: &["write a dump of a running program: its tasks and", "where they are, its counters, its live memory"],
    },
    Command {
        name: "dump",
        usage: &[
            "tondo dump read <file> [why | tasks | stacks | requests | events | sources | task <id>",
            "                       | locals [<task> [<frame>]]",
            "                       | print <path> [<task> [<frame>]] | find <text>",
            "                       | heap | objects | object <address>] [--json]",
        ],
        summary: &["show a dump, as a crash leaves in ~/.cache/tondo/dumps", "or TONDO_DUMP=<dir>"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump diff <file> <file> [--json]"],
        summary: &[
            "what changed from one dump of a program to a later",
            "one: memory by place, tasks, bindings, counters",
        ],
    },
    Command {
        name: "dump",
        usage: &["tondo dump group <dir> [--since <commit>] [--json]"],
        summary: &[
            "the dumps of a directory by the failure they are,",
            "the most frequent first; those new after a commit",
        ],
    },
    Command {
        name: "dump",
        usage: &["tondo dump serve <dir> [--port <n> | --output <file>]"],
        summary: &[
            "the failures of a directory as a page on this machine,",
            "up to date as dumps come; --output writes it once",
        ],
    },
    Command {
        name: "dump",
        usage: &["tondo dump issue <dir> <signature>"],
        summary: &["a failure of a directory of dumps as an issue to", "file, in Markdown"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump test <file> [--frame <n>] [--project <dir>] [--json]"],
        summary: &["a test that calls the function that failed with", "what the dump says it had"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump core <core|pid|-> [--executable <file>] [--output <path>] [--json]"],
        summary: &[
            "a core of the system as a dump: the signal, and",
            "where each task and thread was; by pid from",
            "coredumpctl, - from a pipe, as core_pattern gives it",
        ],
    },
    Command {
        name: "dump",
        usage: &["tondo dump redact <file> <output>"],
        summary: &["the dump without what the program held, to attach", "where anyone reads it"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump report <file> [--output <path>]"],
        summary: &["all a dump says, as one page of HTML"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump trend <dir> [--json]"],
        summary: &["what keeps growing over the dumps of a program in a", "directory, as TONDO_DUMP_WHEN leaves them"],
    },
    Command {
        name: "dump",
        usage: &["tondo dump plugin <lldb|gdb> [--json]"],
        summary: &[
            "write the `tondo` command for lldb or gdb, which",
            "shows a stopped program as `dump read` does, and",
            "a frame's bindings with its `locals` and `print`",
        ],
    },
    Command {
        name: "trace",
        usage: &[
            "tondo trace <pid|name> [--duration <seconds>] [--output <path>] [--via <command>] [--json]",
            "tondo trace [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]",
        ],
        summary: &["record what a program's tasks do, for Perfetto,", "and a profile of where it spends its time"],
    },
    Command {
        name: "profile",
        usage: &[
            "tondo profile <pid|name> [--cpu | --memory | --tasks] [--duration <seconds>] [--output <path>] [--via <command>] [--json]",
            "tondo profile [--cpu | --memory | --tasks] [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]",
            "tondo profile show <profile> [--top <n>] [--lines | --asm [<function>]] [--sample <type>] [--html <path>] [--json]",
        ],
        summary: &[
            "where a program spends its time, what it allocates and",
            "holds, or where its tasks are, in pprof's format; show",
            "reads one: the functions and lines that take the most",
        ],
    },
    Command {
        name: "optimizations",
        usage: &[
            "tondo optimizations [<project>|<file.to>] [--tests] [--function <name>] [--kind <kind>]",
            "          [--against <rev> | --compiler <tondo|rev>] [--json]",
        ],
        summary: &[
            "what a release build makes of the code: the calls it",
            "inlines and those it keeps, the texts and closures made",
            "without the heap, the checks it drops and those it keeps",
        ],
    },
    Command {
        name: "asm",
        usage: &[
            "tondo asm [<project>|<file.to>] [--tests] <function>|--all [--clif] [--rules] [--against <rev> | --compiler <tondo|rev>]",
            "          [--json]",
        ],
        summary: &[
            "the machine code a release build makes of a function, by",
            "the lines of the program, with what the passes did there;",
            "its IR before and after Cranelift optimizes it",
        ],
    },
    Command {
        name: "compiler",
        usage: &["tondo compiler <tondo|rev> [--json]"],
        summary: &[
            "where the tondo of a revision of tondo's repository is,",
            "built once in release, as --compiler takes it",
        ],
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
        usage: &["tondo help [<group> | <command> | --all]"],
        summary: &["the groups of commands; what a group's commands do;", "a command's usage; every usage"],
    },
    Command {
        name: "version",
        usage: &["tondo version [--json]"],
        summary: &["this tondo's version, target and backend"],
    },
];

/// What this `tondo` offers tools beyond its commands, as `tondo version
/// --json` lists it, for a client to check before it relies on one.
pub const FEATURES: &[&str] = &[
    "json",
    "graph",
    "test-events",
    "test-exact-many",
    "test-watch",
    "test-coverage",
    "test-bench",
    "test-fuzz",
    "profile",
    "optimizations",
    "asm",
    "lsp-tests",
    "lsp-lenses",
    "explain",
    "new",
    "add",
    "fix",
];

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

/// A group of commands, by what they are for, as `tondo help` lists them.
pub struct Group {
    pub name: &'static str,
    pub purpose: &'static str,
    pub commands: &'static [&'static str],
}

pub const GROUPS: &[Group] = &[
    Group {
        name: "write",
        purpose: "make, check and build programs",
        commands: &["new", "init", "run", "build", "check", "fix", "fmt"],
    },
    Group { name: "testing", purpose: "test programs and documents", commands: &["test", "doc-test", "model"] },
    Group {
        name: "learn",
        purpose: "learn the language, the library and a program",
        commands: &["guide", "api", "explain", "expand", "graph"],
    },
    Group {
        name: "projects",
        purpose: "dependencies, the cache, C headers and targets",
        commands: &["add", "remove", "update", "clean", "bind", "target"],
    },
    Group {
        name: "observe",
        purpose: "see what a running program does, without stopping it",
        commands: &["ps", "counters", "requests", "blocked", "logpoint", "trace", "dump"],
    },
    Group {
        name: "debugging",
        purpose: "stop, step through and change programs, live or recorded",
        commands: &["debug", "dap", "relay", "source", "reload", "values", "eval"],
    },
    Group {
        name: "performance",
        purpose: "where time and memory go, and what the compiler made",
        commands: &["profile", "optimizations", "asm", "compiler"],
    },
    Group {
        name: "tools",
        purpose: "editors, agents, shells and tondo itself",
        commands: &["lsp", "mcp", "setup", "completions", "help", "version"],
    },
];

/// Where a summary starts, after the usage.
const COLUMN: usize = 55;

const INTRODUCTION: &str = "New to Tondo, or a model writing it? Start with `tondo guide`.";

const NOTES: &str = "\
`tondo help <group>` says what each command of a group does, `tondo help
<command>` or `tondo <command> --help` gives a command's usage, and `tondo
help --all` every usage. `tondo <file.to> [<argument>...]` runs a script (for
#! lines). A project is a directory with src/main.to, or with main.to or
tondo.toml, given as <project> or with --project <dir>; the current
directory by default. With --json, a command writes its result as one JSON
object.";

/// What `tondo help` shows: the groups of commands, each with its own.
pub fn usage() -> String {
    let mut text = format!("{INTRODUCTION}\n\ncommands, by what they are for:\n");
    let width = GROUPS.iter().map(|group| group.name.len()).max().unwrap_or(0) + 2;
    for group in GROUPS {
        text.push_str(&format!("  {:width$}{}\n", group.name, group.purpose));
        text.push_str(&format!("  {:width$}{}\n", "", group.commands.join(", ")));
    }
    text.push('\n');
    text.push_str(NOTES);
    text
}

/// What `tondo help --all` shows: every command's usage.
pub fn all() -> String {
    let mut text = format!("{INTRODUCTION}\n\nusage:\n");
    for command in COMMANDS {
        text.push_str(&entry(command));
    }
    text
}

/// What `tondo help <group>` shows: what each of its commands does, one
/// line each; `None` for no such group.
pub fn group(name: &str) -> Option<String> {
    let group = GROUPS.iter().find(|group| group.name == name)?;
    let entries: Vec<(String, String)> = group
        .commands
        .iter()
        .flat_map(|name| COMMANDS.iter().filter(move |command| command.name == *name))
        .map(|command| (label(command), command.summary.join(" ")))
        .collect();
    let width = entries.iter().map(|(label, _)| label.len()).max().unwrap_or(0) + 2;
    let mut text = format!("{}: {}\n\n", group.name, group.purpose);
    for (label, summary) in entries {
        for (index, line) in wrapped(&summary, WIDTH - width - 2).into_iter().enumerate() {
            let label = if index == 0 { label.as_str() } else { "" };
            text.push_str(&format!("  {label:width$}{line}\n"));
        }
    }
    text.push_str("\n`tondo help <command>` gives a command's usage.\n");
    Some(text)
}

/// How wide `tondo help <group>` writes.
const WIDTH: usize = 100;

/// The words of `text` in lines of `width` at most, but for a longer word.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// A command as a group lists it: its name and the words of its usage
/// that name what it does, as `dump read` or `build --library`.
fn label(command: &Command) -> String {
    let words = command.usage[0].split_whitespace().skip(2);
    let mut label = String::from(command.name);
    for (index, word) in words.enumerate() {
        let plain = word.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !word.starts_with('-');
        if plain || (index == 0 && word.starts_with("--")) {
            label.push(' ');
            label.push_str(word);
        } else {
            break;
        }
        if word.starts_with("--") {
            break;
        }
    }
    label
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
