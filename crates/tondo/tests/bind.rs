//! `tondo bind` on the C library of `tests/bind`: it writes the module
//! checked in there, and a program on that module runs against the library
//! built as a static archive and as a shared library, and against a module
//! read from the library's debug information.

mod common;

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
    run(common::command(compiler).args(["bind", "sample.h", "--output", "sample.to"]).current_dir(&dir));
    let written = std::fs::read_to_string(dir.join("sample.to")).unwrap();
    let expected = std::fs::read_to_string(fixtures().join("sample.to")).unwrap();
    assert_eq!(written, expected, "the module differs from tests/bind/sample.to");
    // As data: what it wrote, and the libraries; outside a project, none
    // is added to a manifest.
    let said = run(common::command(compiler)
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
    let mut bind = common::command(compiler);
    bind.arg("bind");
    if header {
        bind.arg(fixtures().join("sample.h"));
    }
    run(bind.arg("--library").arg(dir.join(library)).arg("--project").arg(&project));
    let executable = dir.join("program");
    run(common::command(compiler).arg("build").arg(&project).arg("--output").arg(&executable));
    // From elsewhere, so a shared library is found through the run-time path.
    run(common::command(&executable).current_dir(std::env::temp_dir()))
}

#[test]
fn static_and_shared() {
    let dir = scratch("bind-libraries");
    let source = fixtures().join("sample.c");
    run(common::command("cc")
        .args(["-c", "-g", "-fPIC", "-o"])
        .arg(dir.join("sample.o"))
        .arg(&source)
        .arg("-I")
        .arg(fixtures()));
    run(common::command("ar").arg("rcs").arg(dir.join("libsample.a")).arg(dir.join("sample.o")));
    let shared = if cfg!(target_os = "macos") { "libsample.dylib" } else { "libsample.so" };
    let mut link = common::command("cc");
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
    run(common::command(compiler).args(["bind", "top.h", "--output", "plain.to"]).current_dir(&dir));
    run(common::command(compiler).args(["bind", "top.h", "--included", "--output", "deep.to"]).current_dir(&dir));
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
    run(common::command(compiler).args(["check"]).current_dir(&dir));
}

/// `--fails` names the functions that fail with the system's error number,
/// which are declared `T ! os.Errno`: of a signed integer or a pointer, the
/// pointer no longer optional, since null is the failure.
#[test]
fn functions_that_fail_with_the_error_number() {
    let dir = scratch("bind-fails");
    let header = "int open_thing(const char *path);\nchar *name_of(int id);\nunsigned count(void);\nint plain(void);\n";
    std::fs::write(dir.join("things.h"), header).unwrap();
    std::fs::write(dir.join("fails.txt"), "# What gives -1 or null with errno.\nopen_thing\nname_of\ncount\n").unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    run(common::command(compiler)
        .args(["bind", "things.h", "--fails", "fails.txt", "--output", "things.to"])
        .current_dir(&dir));
    let module = std::fs::read_to_string(dir.join("things.to")).unwrap();
    for line in [
        "import std.ffi\nimport std.os\n",
        "pub extern fn open_thing(path: ffi.CString?): ffi.CInt ! os.Errno",
        "pub extern fn name_of(id: ffi.CInt): ffi.CString ! os.Errno",
        "pub extern fn plain(): ffi.CInt\n",
        "// - count: it fails with -1 or null only when it gives a signed integer or a pointer",
    ] {
        assert!(module.contains(line), "{line:?} in\n{module}");
    }
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"things\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), format!("{module}\nfn main() {{}}\n")).unwrap();
    run(common::command(compiler).args(["check"]).current_dir(&dir));
}

/// An interface of C, as MIDL writes one for C, is a trait with
/// `@ffi.Interface`: its IID from the header's branch of C++ or from
/// `DEFINE_GUID`, what it extends from the methods its table begins with,
/// and an object of it a `Pointer[ffi.Void]`. A program calls an object C
/// made through it, and asks it for the interface that extends it.
#[test]
fn interfaces_of_c() {
    let dir = scratch("bind-interfaces");
    std::fs::copy(fixtures().join("counter.h"), dir.join("counter.h")).unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    run(common::command(compiler).args(["bind", "counter.h", "--output", "counter.to"]).current_dir(&dir));
    let written = std::fs::read_to_string(dir.join("counter.to")).unwrap();
    let expected = std::fs::read_to_string(fixtures().join("counter.to")).unwrap();
    assert_eq!(written, expected, "the module differs from tests/bind/counter.to");
    run(common::command("cc")
        .args(["-c", "-fPIC", "-o"])
        .arg(dir.join("counter.o"))
        .arg(fixtures().join("counter.c"))
        .arg("-I")
        .arg(fixtures()));
    run(common::command("ar").arg("rcs").arg(dir.join("libcounter.a")).arg(dir.join("counter.o")));
    let project = dir.join("app");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::copy(fixtures().join("interfaces.to"), project.join("src/main.to")).unwrap();
    run(common::command(compiler)
        .arg("bind")
        .arg(dir.join("counter.h"))
        .arg("--library")
        .arg(dir.join("libcounter.a"))
        .arg("--project")
        .arg(&project));
    let executable = dir.join("program");
    run(common::command(compiler).arg("build").arg(&project).arg("--output").arg(&executable));
    assert_eq!(
        run(&mut common::command(&executable)),
        "true true 0x80070057: The parameter is incorrect true 40\ntrue true true 5 tally\n1 0\nadds 2\ntrue 0x80004001: Not implemented 0\n"
    );
    // An interface whose IID the header does not give is left out, and
    // says how to give it; `--id` gives it.
    let header = "#include <stdint.h>\ntypedef int32_t HRESULT;\ntypedef struct IThing IThing;\n\
                  typedef struct IThingVtbl {\n    HRESULT (*QueryInterface)(IThing *This, const void *id, void **found);\n    \
                  uint32_t (*AddRef)(IThing *This);\n    uint32_t (*Release)(IThing *This);\n    \
                  HRESULT (*Touch)(IThing *This);\n} IThingVtbl;\nstruct IThing { const IThingVtbl *lpVtbl; };\n";
    std::fs::write(dir.join("thing.h"), header).unwrap();
    run(common::command(compiler).args(["bind", "thing.h", "--output", "thing.to"]).current_dir(&dir));
    let left = std::fs::read_to_string(dir.join("thing.to")).unwrap();
    assert!(!left.contains("trait IThing"), "{left}");
    assert!(left.contains("// - IThing: no IID found for it: give it with `--id IThing=<GUID>`"), "{left}");
    let id = ["--id", "IThing={8D3C4F2A-1B6E-4A7C-9E05-3F2B1D0C6A94}"];
    run(common::command(compiler).args(["bind", "thing.h", "--output", "thing.to"]).args(id).current_dir(&dir));
    let bound = std::fs::read_to_string(dir.join("thing.to")).unwrap();
    assert!(
        bound.contains("@ffi.Interface(id: \"8d3c4f2a-1b6e-4a7c-9e05-3f2b1d0c6a94\")\npub trait IThing: ffi.Unknown {"),
        "{bound}"
    );
    let wrong =
        common::command(compiler).args(["bind", "thing.h", "--id", "IThing=8d3c"]).current_dir(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("--id takes an interface and its IID"), "{wrong:?}");
}

/// What a header of COM made by MIDL needs of `bind`, as .NET's corprof.h
/// does: its own `IUnknown` is `ffi.Unknown`; a parameter of a function type
/// is a pointer to one; an enumerator past `int`'s values is unsigned, and so
/// is what is made of it; a declaration it cannot read is listed, not left
/// out in silence; and an interface that extends one left out is left out.
#[test]
fn what_a_header_of_com_needs() {
    let dir = scratch("bind-com");
    let header = "#include <stdint.h>\ntypedef int32_t HRESULT;\n#define DEFINE_GUID(name, l, w1, w2, b1, b2, b3, b4, b5, b6, b7, b8) extern int name\n\
        typedef struct IUnknown IUnknown;\n\
        typedef struct IUnknownVtbl {\n    HRESULT (*QueryInterface)(IUnknown *This, const void *id, void **found);\n    \
        uint32_t (*AddRef)(IUnknown *This);\n    uint32_t (*Release)(IUnknown *This);\n} IUnknownVtbl;\n\
        struct IUnknown { const IUnknownVtbl *lpVtbl; };\n\
        typedef int Visit(void *data);\n\
        typedef enum { One = 1, High = 0x80000000, Both = (One | High) } Flags;\n\
        DEFINE_GUID(IID_IWalker, 0x11112222, 0x3333, 0x4444, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc);\n\
        typedef struct IWalker IWalker;\n\
        typedef struct IWalkerVtbl {\n    HRESULT (*QueryInterface)(IWalker *This, const void *id, void **found);\n    \
        uint32_t (*AddRef)(IWalker *This);\n    uint32_t (*Release)(IWalker *This);\n    \
        HRESULT (*Walk)(IWalker *This, Visit visit, Flags flags);\n} IWalkerVtbl;\n\
        struct IWalker { const IWalkerVtbl *lpVtbl; };\n\
        typedef struct IBase IBase;\n\
        typedef struct IBaseVtbl {\n    HRESULT (*QueryInterface)(IBase *This, const void *id, void **found);\n    \
        uint32_t (*AddRef)(IBase *This);\n    uint32_t (*Release)(IBase *This);\n    \
        HRESULT (*Start)(IBase *This);\n} IBaseVtbl;\n\
        struct IBase { const IBaseVtbl *lpVtbl; };\n\
        typedef struct IChild IChild;\n\
        typedef struct IChildVtbl {\n    HRESULT (*QueryInterface)(IChild *This, const void *id, void **found);\n    \
        uint32_t (*AddRef)(IChild *This);\n    uint32_t (*Release)(IChild *This);\n    \
        HRESULT (*Start)(IChild *This);\n    HRESULT (*Grow)(IChild *This);\n} IChildVtbl;\n\
        struct IChild { const IChildVtbl *lpVtbl; };\n\
        struct Broken { Mystery field; };\n";
    std::fs::write(dir.join("com.h"), header).unwrap();
    let compiler = env!("CARGO_BIN_EXE_tondo");
    run(common::command(compiler).args(["bind", "com.h", "--output", "com.to"]).current_dir(&dir));
    let written = std::fs::read_to_string(dir.join("com.to")).unwrap();
    assert!(!written.contains("trait IUnknown"), "{written}");
    assert!(written.contains("pub trait IWalker: ffi.Unknown {"), "{written}");
    assert!(
        written.contains("fn Walk(self, visit: (extern fn(Pointer[ffi.Void]?): ffi.CInt)?, flags: Flags): ffi.HResult"),
        "{written}"
    );
    assert!(
        written.contains("pub alias Flags = ffi.CUInt") && written.contains("pub const Both: Flags = 2147483649"),
        "{written}"
    );
    assert!(written.contains("// - a declaration it could not read, near `Broken{Mystery field;}`"), "{written}");
    // IBase has no IID, and IChild extends it.
    assert!(written.contains("// - IBase: no IID found for it"), "{written}");
    assert!(written.contains("// - IChild: it extends IBase, which is not bound"), "{written}");
    assert!(!written.contains("trait IChild"), "{written}");
    // What it wrote builds.
    let project = dir.join("app");
    std::fs::create_dir_all(project.join("src/com")).unwrap();
    std::fs::write(project.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::copy(dir.join("com.to"), project.join("src/com/com.to")).unwrap();
    let main = "import std.console\nimport app.com\n\nfn main() {\n    console.println(\"{com.IWalkerObject.Id} {com.Both}\")\n}\n";
    std::fs::write(project.join("src/main.to"), main).unwrap();
    let ran = run(common::command(compiler).arg("run").arg(&project));
    assert_eq!(ran, "11112222-3333-4444-5566-778899aabbcc 2147483649\n");
}
