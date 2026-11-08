//! Tondo's own linker (H12): executables it links run, and `--strip`
//! leaves out their symbol table.

#![cfg(target_os = "linux")]

use object::{Object, ObjectSymbol};
use std::path::Path;
use std::process::Command;

fn build(dir: &Path, name: &str, strip: bool) -> Vec<u8> {
    let output = dir.join(name);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tondo"));
    command.args(["build", "main.to", "--output"]).arg(&output).current_dir(dir);
    if strip {
        command.arg("--strip");
    }
    let status = command.status().unwrap();
    assert!(status.success());
    let run = Command::new(&output).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "linked\n");
    std::fs::read(&output).unwrap()
}

#[test]
fn links_and_strips() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.to"), "import std.console\n\nconsole.println(\"linked\")\n").unwrap();
    let full = build(&dir, "full", false);
    let stripped = build(&dir, "stripped", true);
    let file = object::File::parse(full.as_slice()).unwrap();
    assert!(file.symbols().any(|s| s.name() == Ok("tondo_main")));
    let file = object::File::parse(stripped.as_slice()).unwrap();
    assert_eq!(file.symbols().count(), 0);
    assert!(stripped.len() < full.len());
}

/// What the compiler and the linker make does not depend on how their work
/// spreads over the cores, which differs from one build to the next.
#[test]
fn builds_the_same_bytes_each_time() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link_same");
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.json\nimport std.yaml\nimport std.regex\n\npub type Item = { name: String, tags: Array[String] }\n\nlet item = Item { name: \"x\", tags: [\"a\"] }\nlet text = json.encode(item)\nlet _ = yaml.encode(item)\nlet _ = regex.Regex.compile(\"[a-z]+\")\nif text.length() > 0 {\n    console.println(\"linked\")\n}\n";
    std::fs::write(dir.join("main.to"), program).unwrap();
    let first = build(&dir, "first", false);
    let second = build(&dir, "second", false);
    assert!(first == second, "two builds of the same program differ");
}

/// A debug build has the DWARF gdb and lldb read: each function of the
/// program, and the line each run of its code comes from, one for each
/// statement, at the addresses its code has.
#[test]
fn debug_builds_have_their_lines_in_dwarf() {
    use object::ObjectSection;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link-dwarf");
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\n\nfn square(n: Int): Int {\n    let result = n * n\n    result\n}\n\n\
                   fn main() {\n    var total = 0\n    for i in 0..3 {\n        total += square(i)\n    }\n    \
                   console.println(\"total {total}\")\n}\n";
    std::fs::write(dir.join("app.to"), program).unwrap();
    let output = dir.join("app");
    let status = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", "app.to", "--debug", "--output"])
        .arg(&output)
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    let section = |id: gimli::SectionId| -> Result<gimli::EndianSlice<'_, gimli::LittleEndian>, gimli::Error> {
        let data = file.section_by_name(id.name()).and_then(|s| s.data().ok()).unwrap_or(&[]);
        Ok(gimli::EndianSlice::new(data, gimli::LittleEndian))
    };
    let dwarf = gimli::Dwarf::load(section).unwrap();
    let mut units = dwarf.units();
    let header = units.next().unwrap().expect("a unit");
    let unit = dwarf.unit(header).unwrap();
    // The functions, by name and address.
    let mut functions = Vec::new();
    let mut entries = unit.entries();
    while let Some(entry) = entries.next_dfs().unwrap() {
        if entry.tag() == gimli::DW_TAG_subprogram {
            let name = entry.attr_value(gimli::DW_AT_name).unwrap();
            let name = dwarf.attr_string(&unit, name).unwrap().to_string_lossy().into_owned();
            let Some(gimli::AttributeValue::Addr(low)) = entry.attr_value(gimli::DW_AT_low_pc) else { panic!() };
            functions.push((name, low));
        }
    }
    let square = functions.iter().find(|(name, _)| name == "square").map(|&(_, low)| low).expect("square");
    let symbol = file.symbols().find(|s| s.name() == Ok("square")).map(|s| s.address());
    assert!(symbol.is_none_or(|address| address == square), "{functions:?}");
    // A row for each statement's line, the first of `square` at its start.
    let program = unit.line_program.clone().unwrap();
    let mut rows = program.rows();
    let mut lines = Vec::new();
    while let Some((_, row)) = rows.next_row().unwrap() {
        if !row.end_sequence() {
            lines.push((row.address(), row.line().map_or(0, |l| l.get())));
        }
    }
    for line in [4, 5, 9, 10, 11, 13] {
        assert!(lines.iter().any(|&(_, l)| l == line), "line {line}: {lines:?}");
    }
    assert!(lines.contains(&(square, 4)), "{lines:?}");
}

/// gdb stops a debug build at a line of its DWARF, and the `tondo` command
/// that `tondo dump plugin gdb` writes shows the stopped task's stack, from
/// a dump the program writes when gdb asks it to.
#[test]
fn gdb_stops_at_a_line_and_shows_the_tasks() {
    if Command::new("gdb").arg("--version").output().is_err() {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link-gdb");
    std::fs::create_dir_all(&dir).unwrap();
    let program = "fn square(n: Int): Int {\n    let result = n * n\n    result\n}\n\nfn main() {\n    \
                   _ = square(3)\n}\n";
    std::fs::write(dir.join("app.to"), program).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let status = Command::new(tondo).args(["build", "app.to", "--debug", "--output", "app"]).current_dir(&dir).status();
    assert!(status.unwrap().success());
    let plugin = Command::new(tondo).args(["dump", "plugin", "gdb"]).env("XDG_CACHE_HOME", &dir).output().unwrap();
    let load = String::from_utf8_lossy(&plugin.stdout).trim().to_string();
    assert!(load.starts_with("source "), "{load}");
    let ran = Command::new("gdb")
        .args(["-q", "-batch", "-nx", "-ex", "set debuginfod enabled off", "-ex", &load])
        .args(["-ex", "break app.to:2", "-ex", "run", "-ex", "bt", "-ex", "tondo task 1", "-ex", "tondo locals"])
        .args(["-ex", "up", "-ex", "tondo print 3 * 2", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&ran.stdout);
    // An emulator does not let gdb trace a process.
    if shown.trim().is_empty() {
        return;
    }
    assert!(shown.contains("square () at app.to:2"), "{shown}");
    assert!(shown.contains("in main () at app.to:7"), "{shown}");
    assert!(
        shown.contains("task 1 (main) runs at app.to:2 in square") && shown.contains("app.to:7 in main"),
        "{shown}"
    );
    // The bindings of the frame gdb shows, as the debugger of Tondo shows them.
    assert!(shown.contains("n: Int = 3"), "{shown}");
    assert!(shown.contains("3 * 2 = 6"), "{shown}");
}

/// A C library whose objects each add an entry to a section, as C code
/// registers what it has, and walks them from `__start_` to `__stop_` of
/// the section: the linker keeps the entries, though nothing names them,
/// together, between the two.
#[test]
fn sections_are_bounded_by_start_and_stop() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link_bounds");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let entry = "struct entry { const char *name; int value; };\n";
    let first = format!(
        "{entry}__attribute__((used, section(\"tondo_entries\"))) static const struct entry first = {{ \"first\", 1 }};\n\
         extern const struct entry __start_tondo_entries[], __stop_tondo_entries[];\n\
         int second_marker(void);\n\
         int entries_count(void) {{ return second_marker() + (int)(__stop_tondo_entries - __start_tondo_entries); }}\n\
         int entries_sum(void) {{\n    int sum = 0;\n    for (const struct entry *e = __start_tondo_entries; e < __stop_tondo_entries; e++) sum += e->value;\n    return sum;\n}}\n"
    );
    let second = format!(
        "{entry}static int unrelated = 7;\n\
         __attribute__((used, section(\"tondo_entries\"))) static const struct entry second = {{ \"second\", 41 }};\n\
         int second_marker(void) {{ return unrelated - 7; }}\n"
    );
    for (name, source) in [("first", first), ("second", second)] {
        std::fs::write(dir.join(format!("{name}.c")), source).unwrap();
        let compiled = Command::new("cc")
            .args(["-c", "-fPIC", &format!("{name}.c"), "-o", &format!("{name}.o")])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    }
    let archived = Command::new("ar").args(["rcs", "libentries.a", "first.o", "second.o"]).current_dir(&dir).status();
    assert!(archived.unwrap().success());
    let library = dir.join("libentries.a");
    let manifest = format!("[package]\nname = \"entries\"\n\n[native]\nlibraries = [\"{}\"]\n", library.display());
    std::fs::write(dir.join("tondo.toml"), manifest).unwrap();
    let program = "import std.console\nimport std.ffi\n\nextern fn entries_count(): ffi.CInt\nextern fn entries_sum(): ffi.CInt\n\n\
                   fn main() {\n    let (count, sum) = unsafe { (entries_count(), entries_sum()) }\n    console.println(\"{count} {sum}\")\n}\n";
    std::fs::write(dir.join("src/main.to"), program).unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", ".", "--output", "entries"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = Command::new(dir.join("entries")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "2 42\n");
}

/// A C library may call `atexit`, which glibc's shared library leaves to
/// `libc_nonshared.a`, as it does to `pthread_atfork`.
#[test]
fn a_c_library_may_call_atexit() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link_atexit");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let source = "#include <stdio.h>\n#include <stdlib.h>\n\nstatic void bye(void) { puts(\"bye\"); }\n\n\
                  int farewell(void) { return atexit(bye); }\n";
    std::fs::write(dir.join("farewell.c"), source).unwrap();
    let compiled =
        Command::new("cc").args(["-c", "-fPIC", "farewell.c", "-o", "farewell.o"]).current_dir(&dir).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let archived = Command::new("ar").args(["rcs", "libfarewell.a", "farewell.o"]).current_dir(&dir).status();
    assert!(archived.unwrap().success());
    let library = dir.join("libfarewell.a");
    let manifest = format!("[package]\nname = \"farewell\"\n\n[native]\nlibraries = [\"{}\"]\n", library.display());
    std::fs::write(dir.join("tondo.toml"), manifest).unwrap();
    let program = "import std.console\nimport std.ffi\n\nextern fn farewell(): ffi.CInt\n\n\
                   fn main() {\n    let registered = unsafe { farewell() }\n    console.println(\"registered {registered}\")\n}\n";
    std::fs::write(dir.join("src/main.to"), program).unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["build", ".", "--output", "farewell"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = Command::new(dir.join("farewell")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "registered 0\nbye\n");
}
