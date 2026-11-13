//! `tondo bind` on the C library of `tests/bind`: it writes the module
//! checked in there, and a program on that module runs against the library
//! built as a static archive and as a shared library, and against a module
//! read from the library's debug information.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/bind")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn module() {
    let dir = scratch("bind-module");
    std::fs::copy(fixtures().join("sample.h"), dir.join("sample.h")).unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    run(Command::new(compiler).args(["bind", "sample.h", "--output", "sample.to"]).current_dir(&dir));
    let written = std::fs::read_to_string(dir.join("sample.to")).unwrap();
    let expected = std::fs::read_to_string(fixtures().join("sample.to")).unwrap();
    assert_eq!(written, expected, "the module differs from tests/bind/sample.to");
    // As data: what it wrote, and the libraries; outside a project, none
    // is added to a manifest.
    let said = run(Command::new(compiler)
        .args(["bind", "sample.h", "--output", "again.to", "--library", "m", "--json"])
        .current_dir(&dir));
    let said: serde_json::Value = serde_json::from_str(&said).unwrap();
    assert_eq!(
        said,
        serde_json::json!({
            "format": "tondo-bind-0.1/1", "ok": true, "file": "again.to", "module": "again",
            "libraries": ["m"], "added": [],
        })
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("again.to")).unwrap(),
        written.replace("--output sample.to", "--output again.to --library m --json")
    );
}

/// Builds a project with the program `main` that binds the library through
/// `library`, a path in `dir`, from its header or else its debug
/// information, and runs the program.
fn program(dir: &Path, library: &str, main: &str, header: bool) -> String {
    let compiler = env!("CARGO_BIN_EXE_tondo");
    let project = dir.join("app");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::copy(fixtures().join(main), project.join("src/main.to")).unwrap();
    let mut bind = Command::new(compiler);
    bind.arg("bind");
    if header {
        bind.arg(fixtures().join("sample.h"));
    }
    run(bind.arg("--library").arg(dir.join(library)).arg("--project").arg(&project));
    let executable = dir.join("program");
    run(Command::new(compiler).arg("build").arg(&project).arg("--output").arg(&executable));
    // From elsewhere, so a shared library is found through the run-time path.
    run(Command::new(&executable).current_dir(std::env::temp_dir()))
}

#[test]
fn static_and_shared() {
    let dir = scratch("bind-libraries");
    let source = fixtures().join("sample.c");
    run(Command::new("cc")
        .args(["-c", "-g", "-fPIC", "-o"])
        .arg(dir.join("sample.o"))
        .arg(&source)
        .arg("-I")
        .arg(fixtures()));
    run(Command::new("ar").arg("rcs").arg(dir.join("libsample.a")).arg(dir.join("sample.o")));
    let shared = if cfg!(target_os = "macos") { "libsample.dylib" } else { "libsample.so" };
    let mut link = Command::new("cc");
    if cfg!(target_os = "macos") {
        link.args(["-dynamiclib", "-install_name", "@rpath/libsample.dylib"]);
    } else {
        link.args(["-shared", "-Wl,-soname,libsample.so"]);
    }
    run(link.arg("-o").arg(dir.join(shared)).arg(dir.join("sample.o")));
    let expected = "42 2 1 30\nx-42 16.0 5\n99 42 1.2 5000000000\n-2.5 -7 log=5 1 0\n5 17 -3 true 2.0\n28 17\n";
    let static_dir = scratch("bind-static");
    std::fs::copy(dir.join("libsample.a"), static_dir.join("libsample.a")).unwrap();
    assert_eq!(program(&static_dir, "libsample.a", "main.to", true), expected);
    let shared_dir = scratch("bind-shared");
    std::fs::copy(dir.join(shared), shared_dir.join(shared)).unwrap();
    assert_eq!(program(&shared_dir, shared, "main.to", true), expected);
    let debug_dir = scratch("bind-debug");
    std::fs::copy(dir.join("libsample.a"), debug_dir.join("libsample.a")).unwrap();
    assert_eq!(program(&debug_dir, "libsample.a", "dwarf.to", false), "42 2 1 30\n99 42 6 -2.5\n");
}

/// `--included` binds what a header includes too, as a system's headers keep
/// their numbers in others; a name a header keeps for itself, `__size_t`,
/// is written as what it names; and an enum with a value past `int`'s is
/// unsigned, as C makes it.
#[test]
fn included_headers() {
    let dir = scratch("bind-included");
    std::fs::create_dir_all(dir.join("bits")).unwrap();
    std::fs::write(dir.join("bits/inner.h"), "typedef unsigned int __inner_size_t;\n#define INNER_LIMIT 7\n").unwrap();
    let top = "#include \"bits/inner.h\"\n__inner_size_t measure(const char *text);\nenum bits { BIT_LOW = 1, BIT_HIGH = 2147483648u };\n";
    std::fs::write(dir.join("top.h"), top).unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    run(Command::new(compiler).args(["bind", "top.h", "--output", "plain.to"]).current_dir(&dir));
    run(Command::new(compiler).args(["bind", "top.h", "--included", "--output", "deep.to"]).current_dir(&dir));
    let plain = std::fs::read_to_string(dir.join("plain.to")).unwrap();
    let deep = std::fs::read_to_string(dir.join("deep.to")).unwrap();
    assert!(!plain.contains("INNER_LIMIT") && deep.contains("pub const INNER_LIMIT: ffi.CInt = 7"), "{deep}");
    for module in [&plain, &deep] {
        assert!(module.contains("pub extern fn measure(text: ffi.CString?): ffi.CUInt"), "{module}");
        assert!(module.contains("pub alias bits = ffi.CUInt"), "{module}");
        assert!(!module.contains("__inner_size_t"), "{module}");
    }
    // It checks as it is written.
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"deep\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), format!("{deep}\nfn main() {{}}\n")).unwrap();
    run(Command::new(compiler).args(["check"]).current_dir(&dir));
}
