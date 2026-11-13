//! Libraries for C (toolchain 7.7): `tondo build --library` writes a
//! shared and a static library and their header, which a C program
//! compiles and links with the system's C compiler, or loads as it runs,
//! and calls from its threads.

use std::path::{Path, PathBuf};
use std::process::Command;

const LIBRARY: &str = r#"import std.console
import std.ffi
import std.time

/// A point of the plane.
pub extern type Point = {
    x: Float
    y: Float
}

/// The words of `text`, separated by spaces.
pub extern fn words_count(text: ffi.CString): ffi.CSize {
    UInt64(unsafe { ffi.fromCString(text) }.split(" ").length()) ?? 0u64
}

/// The distance between two points.
pub extern fn words_distance(a: Point, b: Point): Float {
    let (dx, dy) = (a.x - b.x, a.y - b.y)
    (dx * dx + dy * dy).checkedSqrt() ?? 0.0
}

/// Calls `visit` with each number from 1 to `count`.
pub extern fn words_each(count: ffi.CInt, visit: extern fn(ffi.CInt): Unit) {
    for i in 1..=count {
        unsafe { visit(i) }
    }
}

/// The sum of 1 to `n`, made by a task after a pause.
pub extern fn words_sum(n: Int64): Int64 {
    let job = spawn total(n)
    job.wait()
}

pub extern fn words_hello(name: ffi.CString) {
    console.println("hello, {unsafe { ffi.fromCString(name) }}")
}

/// Writes `text` with dashes for its spaces into `out`, which holds
/// `capacity` bytes, and gives its length.
pub extern fn words_dashed(text: ffi.CString, out: Pointer[ffi.CChar], capacity: ffi.CSize): ffi.CSize {
    let bytes = unsafe { ffi.fromCString(text) }.replace(" ", "-").bytes()
    let room = Int(capacity) ?? 0
    if room > 0 {
        let fits = bytes.length().min(room - 1)
        unsafe {
            for index in 0..fits {
                out.offset(index).write(bytes[index])
            }
            out.offset(fits).write(Byte(0u8))
        }
    }
    UInt64(bytes.length()) ?? 0u64
}

/// Calls `each` with each word of `text`.
pub extern fn words_split(text: ffi.CString, each: extern fn(ffi.CString): Unit) {
    for word in unsafe { ffi.fromCString(text) }.split(" ") {
        unsafe { each(word) }
    }
}

fn total(n: Int64): Int64 {
    time.sleep(time.milliseconds(2))
    var sum = 0
    for i in 1..=n {
        sum = sum + i
    }
    sum
}
"#;

const PROGRAM: &str = r#"#include <pthread.h>
#include <stdio.h>
#include "words.h"

static void show(int i) { printf("visit %d\n", i); }

static void word(const char *w) { printf("word %s\n", w); }

static void *work(void *arg) {
    long id = (long)arg;
    size_t words = 0;
    for (int i = 0; i < 1000; i++) words += words_count("a b c");
    long long sum = words_sum(100);
    return (void *)(long)(words == 3000 && sum == 5050);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("count %zu\n", words_count("one two three"));
    Point a = {0, 0}, b = {3, 4};
    printf("distance %g\n", words_distance(a, b));
    words_each(2, show);
    words_hello("C");
    char dashed[16];
    printf("dashed %zu %s\n", words_dashed("a b c", dashed, sizeof dashed), dashed);
    words_split("x y", word);
    pthread_t threads[4];
    for (long i = 0; i < 4; i++) pthread_create(&threads[i], NULL, work, (void *)i);
    int right = 0;
    for (int i = 0; i < 4; i++) {
        void *result;
        pthread_join(threads[i], &result);
        right += (int)(long)result;
    }
    printf("threads %d\n", right);
    return 0;
}
"#;

fn tondo(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap()
}

fn project(name: &str, source: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("library").join(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src").join(format!("{name}.to")), source).unwrap();
    dir
}

#[test]
fn a_c_program_calls_a_library() {
    let dir = project("words", LIBRARY);
    let built = tondo(&dir, &["build", "--library", "--output", "out"]);
    let said = String::from_utf8_lossy(&built.stdout).into_owned();
    assert!(built.status.success(), "{said}{}", String::from_utf8_lossy(&built.stderr));
    let header = std::fs::read_to_string(dir.join("out/words.h")).unwrap();
    for declaration in [
        "typedef struct Point Point;",
        "/** The words of `text`, separated by spaces. */\nsize_t words_count(const char *text);",
        "double words_distance(Point a, Point b);",
        "void words_each(int count, void (*visit)(int));",
        // A text it reads is const, and a buffer it writes is not.
        "size_t words_dashed(const char *text, char *out, size_t capacity);",
        "void words_split(const char *text, void (*each)(const char *));",
    ] {
        assert!(header.contains(declaration), "{declaration} in\n{header}");
    }
    // The lines the build prints link it, shared or static.
    std::fs::write(dir.join("app.c"), PROGRAM).unwrap();
    for (prefix, program) in
        [("Link the shared library: cc app.c ", "shared"), ("or the static one: cc app.c ", "static")]
    {
        let line = said.lines().find_map(|l| l.strip_prefix(prefix)).unwrap();
        let compiled = Command::new("cc")
            .args(["app.c", "-Iout", "-Werror=incompatible-pointer-types", "-o", program])
            .args(line.split(' '))
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
        let run = Command::new(dir.join(program)).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            "count 3\ndistance 5\nvisit 1\nvisit 2\nhello, C\ndashed 5 a-b-c\nword x\nword y\nthreads 4\n",
            "{program}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    // A program that loads it when it runs finds what it publishes, and
    // nothing else.
    let shared = dir.join(if cfg!(target_os = "macos") { "out/libwords.dylib" } else { "out/libwords.so" });
    std::fs::write(dir.join("load.c"), LOADER).unwrap();
    let compiled = Command::new("cc").args(["load.c", "-o", "load"]).current_dir(&dir).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let run = Command::new(dir.join("load")).arg(&shared).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "5050 2 hidden\n", "{}", String::from_utf8_lossy(&run.stderr));
}

/// A project of several modules writes its `extern type`s in the header by
/// the names they were declared with, which C takes, and is named after its
/// package, not its directory.
#[test]
fn a_project_of_modules_publishes_its_types_by_their_names() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("library").join("several");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/text")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"textkit\"\n").unwrap();
    std::fs::write(
        dir.join("src/textkit.to"),
        "import std.ffi\nimport textkit.text\n\npub extern type Stats = {\n    words: ffi.CLong\n}\n\n\
         pub extern fn textkit_stats(input: ffi.CString): Stats {\n    \
         Stats { words: text.words(unsafe { ffi.fromCString(input) }) }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/text/text.to"),
        "pub fn words(line: String): Int {\n    line.split(\" \").length()\n}\n",
    )
    .unwrap();
    let built = tondo(&dir, &["build", "--library", "--output", "out"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let header = std::fs::read_to_string(dir.join("out/textkit.h")).unwrap();
    for declaration in ["typedef struct Stats Stats;", "struct Stats {", "Stats textkit_stats(const char *input);"] {
        assert!(header.contains(declaration), "{declaration} in\n{header}");
    }
    std::fs::write(
        dir.join("app.c"),
        "#include \"textkit.h\"\n#include <stdio.h>\n\
         int main(void) { printf(\"%ld\\n\", (long)textkit_stats(\"a b c\").words); return 0; }\n",
    )
    .unwrap();
    let rpath = format!("-Wl,-rpath,{}", dir.join("out").display());
    let compiled = Command::new("cc")
        .args(["app.c", "-Iout", "-Lout", "-ltextkit", &rpath, "-o", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let run = Command::new(dir.join("app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "3\n", "{}", String::from_utf8_lossy(&run.stderr));
}

const LOADER: &str = r#"#include <dlfcn.h>
#include <stdio.h>

int main(int argc, char **argv) {
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!library) {
        printf("%s\n", dlerror());
        return 1;
    }
    long long (*sum)(long long) = (long long (*)(long long))dlsym(library, "words_sum");
    size_t (*count)(const char *) = (size_t (*)(const char *))dlsym(library, "words_count");
    void *runtime = dlsym(library, "tondo_rt_callback");
    printf("%lld %zu %s\n", sum(100), count("x y"), runtime ? "visible" : "hidden");
    return 0;
}
"#;

#[test]
fn a_library_publishes_functions_and_is_checked_and_tested_as_one() {
    let dir = project("empty", "fn helper(): Int {\n    1\n}\n");
    let checked = tondo(&dir, &["check", "--library"]);
    assert!(!checked.status.success());
    assert!(String::from_utf8_lossy(&checked.stderr).contains("E1807"));
    let program = tondo(&dir, &["build"]);
    assert!(String::from_utf8_lossy(&program.stderr).contains("a library for C builds with `--library`"));

    let dir = project("tested", "pub extern fn tested_one(): Int {\n    1\n}\n");
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    let test = "import tested\n\ntest one {\n    assert(tested.tested_one() == 1)\n}\n";
    std::fs::write(dir.join("tests/one.to"), test).unwrap();
    assert!(tondo(&dir, &["check", "--library"]).status.success());
    let tested = tondo(&dir, &["test", "--library"]);
    assert!(tested.status.success(), "{}", String::from_utf8_lossy(&tested.stdout));
}

const ENDS: &str = r#"import std.console
import std.net
import std.time

/// Twice a number.
pub extern fn ends_twice(n: Int64): Int64 {
    n * 2
}

/// Starts tasks that run until the library ends: one sleeps, one waits
/// for connections, one asks the network.
pub extern fn ends_start(): Int64 {
    spawn detached sleeping()
    match net.listen("127.0.0.1", 0) {
        ok(listener) => {
            let port = listener.address().port
            spawn detached accepting(listener)
            port
        }
        err(_) => 0
    }
}

fn sleeping() {
    defer {
        console.println("cleaned")
    }
    for {
        time.sleep(time.milliseconds(5))
    }
}

fn accepting(listener: net.Listener) {
    for {
        match listener.accept() {
            ok(stream) => _ = stream.close()
            err(_) => return
        }
    }
}
"#;

const UNLOADER: &str = r#"#include <dlfcn.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>

static long long (*twice)(long long);

static void *call(void *argument) {
    printf("from a thread %lld\n", twice(4));
    return NULL;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    for (int round = 0; round < 2; round++) {
        void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
        twice = (long long (*)(long long))dlsym(library, "ends_twice");
        long long (*start)(void) = (long long (*)(void))dlsym(library, "ends_start");
        void (*stop)(void) = (void (*)(void))dlsym(library, "ends_stop");
        printf("round %d: %lld, listening %d\n", round, twice(21), start() > 0);
        if (round == 1) {
            pthread_t thread;
            pthread_create(&thread, NULL, call, NULL);
            pthread_join(thread, NULL);
        }
        stop();
        struct sigaction fault;
        sigaction(SIGSEGV, NULL, &fault);
        printf("stopped, the fault's handler %s\n", fault.sa_handler == SIG_DFL ? "the process's" : "the library's");
        if (round == 0) {
            printf("again %lld, listening %d\n", twice(2), start() > 0);
            stop();
            stop();
        }
        dlclose(library);
        printf("%s\n", dlopen(argv[1], RTLD_NOW | RTLD_NOLOAD) ? "loaded" : "unloaded");
    }
    return 0;
}
"#;

/// A library that Tondo built ends with `<name>_stop` (toolchain 7.7):
/// its tasks run their `defer`s, its threads end, the handlers it
/// installed give way, and the system unloads it; a later call starts it
/// again, and a library loaded again starts a new runtime.
#[test]
fn a_library_that_tondo_built_ends_and_unloads() {
    let dir = project("ends", ENDS);
    let built = tondo(&dir, &["build", "--library", "--output", "out"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let header = std::fs::read_to_string(dir.join("out/ends.h")).unwrap();
    assert!(header.contains("\nvoid ends_stop(void);\n"), "{header}");
    std::fs::write(dir.join("unload.c"), UNLOADER).unwrap();
    let compiled =
        Command::new("cc").args(["unload.c", "-o", "unload", "-lpthread"]).current_dir(&dir).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let shared = dir.join(if cfg!(target_os = "macos") { "out/libends.dylib" } else { "out/libends.so" });
    let run = Command::new(dir.join("unload")).arg(&shared).output().unwrap();
    // macOS keeps an image with thread-local variables loaded, which the
    // runtime has, and musl never unloads one: there the library ends,
    // stays, and starts again.
    let kept = cfg!(any(target_os = "macos", target_env = "musl"));
    let unloaded = if kept { "loaded" } else { "unloaded" };
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        format!(
            "round 0: 42, listening 1\ncleaned\nstopped, the fault's handler the process's\nagain 4, listening 1\ncleaned\n{unloaded}\n\
             round 1: 42, listening 1\nfrom a thread 8\ncleaned\nstopped, the fault's handler the process's\n{unloaded}\n"
        ),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.status.success());
    // A program of Tondo ends it before it unloads it, by itself when the
    // last holder goes or when it closes it.
    let host = project("endsHost", "");
    std::fs::remove_file(host.join("src/endsHost.to")).unwrap();
    std::fs::write(host.join("src/main.to"), ENDS_HOST).unwrap();
    let built = tondo(&host, &["build", "--output", "host"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let no_load = if cfg!(target_os = "macos") { "16" } else { "4" };
    let run = Command::new(host.join("host")).arg(&shared).arg(no_load).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        format!(
            "cleaned\nlistening true, loaded {kept}\ncleaned\nlistening true, loaded {kept}\n10\nclosed, loaded {kept}\n"
        ),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

const ENDS_HOST: &str = r#"import std.console
import std.env
import std.ffi

/// Whether the system has the library loaded.
fn loaded(path: String, noLoad: Int32): Bool ! ffi.LibraryError {
    let program = ffi.Library.program()
    let open = unsafe { program.function[extern fn(ffi.CString, ffi.CInt): Pointer[UInt8]]("dlopen")? }
    let handle = unsafe { open(path, 2i32 + noLoad) }
    unsafe { handle.address() } != 0
}

fn round(path: String): Int64 ! ffi.LibraryError {
    let ends = ffi.Library.open(path)?
    let start = unsafe { ends.function[extern fn(): Int64]("ends_start")? }
    unsafe { start() }
}

fn main(): !ffi.LibraryError {
    let (path, noLoad) = (env.args()[0], if env.args()[1] == "16" { 16i32 } else { 4i32 })
    // The program's output and the library's are two runtimes' own: each
    // line goes out before the library runs again.
    console.println("listening {round(path)? > 0}, loaded {loaded(path, noLoad)?}")
    _ = console.flush()
    console.println("listening {round(path)? > 0}, loaded {loaded(path, noLoad)?}")
    _ = console.flush()
    let ends = ffi.Library.open(path)?
    let twice = unsafe { ends.function[extern fn(Int64): Int64]("ends_twice")? }
    console.println("{unsafe { twice(5) }}")
    unsafe { ends.close()? }
    console.println("closed, loaded {loaded(path, noLoad)?}")
}
"#;

/// A program loads a library as it runs (standard library 19.3): by its
/// path from the current directory, or by its name where the system looks;
/// it calls a function of it, asks for what it publishes, closes it, and
/// is told why when it cannot. A static build loads none.
#[test]
fn a_program_loads_a_library_as_it_runs() {
    let plugin = project(
        "plugin",
        "import std.ffi\n\n/// Twice a number.\npub extern fn plugin_twice(n: ffi.CLong): ffi.CLong {\n    n * 2\n}\n",
    );
    let built = tondo(&plugin, &["build", "--library", "--output", "out"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let host = project("host", "");
    std::fs::remove_file(host.join("src/host.to")).unwrap();
    // A value the C library publishes, where it publishes it by this name.
    let environ = if cfg!(target_os = "linux") {
        "    let environment = unsafe { program.pointer[UInt64](\"environ\")? }\n    console.println(\"{unsafe { environment.read() } != 0u64}\")\n"
    } else {
        "    console.println(\"true\")\n"
    };
    let program = format!(
        r#"import std.console
import std.ffi
import std.os

fn asked(found: Bool ! ffi.LibraryError): String {{
    match found {{
        ok(_) => "ok"
        err(ffi.LibraryError.NotLoaded(why)) => "not loaded: {{why.length() > 0}}"
        err(error) => "{{error}}"
    }}
}}

fn main(): !ffi.LibraryError {{
    let file = "../plugin/out/{{ffi.libraryFileName("plugin")}}"
    let plugin = ffi.Library.open(file)?
    let twice = unsafe {{ plugin.function[extern fn(ffi.CLong): ffi.CLong]("plugin_twice")? }}
    console.println("{{unsafe {{ twice(21) }}}} {{plugin.has("plugin_twice")}} {{plugin.has("plugin_thrice")}}")
    console.println("{{plugin.path().endsWith(ffi.libraryFileName("plugin"))}} {{unsafe {{ twice.raw()(2) }}}}")
    let named = ffi.Library.named("plugin")?
    let found = unsafe {{ named.function[extern fn(ffi.CLong): ffi.CLong]("plugin_twice")? }}
    console.println("{{unsafe {{ found(4) }}}}")
    let missing = unsafe {{ plugin.function[extern fn(): ffi.CInt]("plugin_thrice") }}
    console.println(asked(missing.map((_) {{ true }})))
    console.println(asked(ffi.Library.open("nowhere.so").map((_) {{ true }})))
    console.println(asked(ffi.Library.open("src/main.to").map((_) {{ true }})))
    let program = ffi.Library.program()
    console.println("{{program.has("strlen")}}")
    // A C function of it that fails with the system's error number.
    let change = unsafe {{ program.function[extern fn(ffi.CString): ffi.CInt ! os.Errno]("chdir")? }}
    console.println("{{unsafe {{ change("/no/such") }}}} {{unsafe {{ change.raw()(".") }}}}")
    // And as `toFunction` gives it, from its address.
    let address = unsafe {{ program.pointer[UInt8]("chdir")?.address() }}
    let fromAddress = unsafe {{ address.toFunction[extern fn(ffi.CString): ffi.CInt ! os.Errno]() }}
    console.println("{{unsafe {{ fromAddress("/no/such") }}}}")
{environ}    unsafe {{ plugin.close()? }}
    console.println("{{plugin.has("plugin_twice")}}")
    console.println(asked(unsafe {{ plugin.function[extern fn(): ffi.CInt]("plugin_twice") }}.map((_) {{ true }})))
    console.println(asked(unsafe {{ plugin.close() }}.map((_) {{ true }})))
    console.println(asked(unsafe {{ program.close() }}.map((_) {{ true }})))
}}
"#
    );
    std::fs::write(host.join("src/main.to"), program).unwrap();
    let built = tondo(&host, &["build", "--output", "host"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let search = if cfg!(target_os = "macos") { "DYLD_LIBRARY_PATH" } else { "LD_LIBRARY_PATH" };
    let run = Command::new(host.join("host")).current_dir(&host).env(search, plugin.join("out")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "42 true false\ntrue 4\n8\nthe library publishes no `plugin_thrice`\nnot loaded: true\nnot loaded: true\ntrue\nerr(os error 2: No such file or directory) ok(0)\nerr(os error 2: No such file or directory)\ntrue\nfalse\nthe library was closed\nthe library was closed\nthe library was not unloaded: the program itself is never unloaded\n",
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    // `function` takes an `extern fn` type.
    std::fs::write(host.join("src/main.to"), "import std.ffi\n\nfn main() {\n    let program = ffi.Library.program()\n    _ = unsafe { program.function[Int](\"strlen\") }\n}\n").unwrap();
    let checked = tondo(&host, &["check"]);
    let said = String::from_utf8_lossy(&checked.stderr);
    assert!(said.contains("E1104") && said.contains("its type argument is an `extern fn(...)`"), "{said}");
    // A static executable loads no library.
    if cfg!(target_os = "linux") {
        std::fs::write(host.join("src/main.to"), "import std.ffi\n\nfn main() {\n    _ = ffi.Library.program()\n}\n")
            .unwrap();
        let built = tondo(&host, &["build", "--static", "--output", "static"]);
        assert!(String::from_utf8_lossy(&built.stderr).contains("a static executable loads no library"));
        let machine = if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" };
        let built = tondo(&host, &["build", "--target", machine, "--output", "static"]);
        assert!(String::from_utf8_lossy(&built.stderr).contains("and a build with --target"));
    }
}
