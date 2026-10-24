//! Libraries for C (toolchain 7): `tondo build --library` writes a static
//! library and its header, which a C program compiles and links with the
//! system's C compiler, and calls from its threads.

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
    UInt64(unsafe { ffi.fromCString(text) }.split(" ").length()).unwrapOr(0u64)
}

/// The distance between two points.
pub extern fn words_distance(a: Point, b: Point): Float {
    let (dx, dy) = (a.x - b.x, a.y - b.y)
    (dx * dx + dy * dy).sqrt().unwrapOr(0.0)
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
        "/** The words of `text`, separated by spaces. */\nsize_t words_count(char *text);",
        "double words_distance(Point a, Point b);",
        "void words_each(int count, void (*visit)(int));",
    ] {
        assert!(header.contains(declaration), "{declaration} in\n{header}");
    }
    // The line the build prints links it.
    let line = said.lines().find_map(|l| l.strip_prefix("Link it: cc app.c ")).unwrap();
    std::fs::write(dir.join("app.c"), PROGRAM).unwrap();
    let compiled = Command::new("cc")
        .args(["app.c", "-Iout", "-o", "app"])
        .args(line.split(' '))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let run = Command::new(dir.join("app")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "count 3\ndistance 5\nvisit 1\nvisit 2\nhello, C\nthreads 4\n",
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn a_library_publishes_functions_and_is_checked_and_tested_as_one() {
    let dir = project("empty", "fn helper(): Int {\n    1\n}\n");
    let checked = tondo(&dir, &["check", "--library"]);
    assert!(!checked.status.success());
    assert!(String::from_utf8_lossy(&checked.stderr).contains("E1807"));
    let program = tondo(&dir, &["check"]);
    assert!(String::from_utf8_lossy(&program.stderr).contains("a library for C builds with `--library`"));

    let dir = project("tested", "pub extern fn tested_one(): Int {\n    1\n}\n");
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    let test = "import tested\n\ntest one {\n    assert(tested.tested_one() == 1)\n}\n";
    std::fs::write(dir.join("tests/one.to"), test).unwrap();
    assert!(tondo(&dir, &["check", "--library"]).status.success());
    let tested = tondo(&dir, &["test", "--library"]);
    assert!(tested.status.success(), "{}", String::from_utf8_lossy(&tested.stdout));
}
