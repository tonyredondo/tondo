//! `tondo test` on the project of `tests/testing/sample`: what it runs
//! and reports, what it selects, and the errors a test file can have.

use std::path::{Path, PathBuf};
use std::process::Command;

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/testing/sample")
}

/// Runs `tondo` in `dir`: stdout, stderr and the exit status.
fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

/// The report without the time the run took.
fn untimed(report: &str) -> String {
    match report.rsplit_once("; ") {
        Some((head, time)) if time.trim_end().ends_with('s') => format!("{head}\n"),
        _ => report.to_string(),
    }
}

const REPORT: &str = "\
test sample::integration::api::parsesThroughThePublicApi ... ok
test sample::integration::api::rejectsEmptyText ... ok
test sample::unit::sample::brokenSetup::isBlocked ... BLOCKED: its suite sample::unit::sample::brokenSetup failed
test sample::unit::sample::doublesPrivately ... ok
test sample::unit::sample::failsAnAssertion ... FAILED
test sample::unit::sample::logsAndTags ... ok
test sample::unit::sample::returnsAnError ... FAILED
test sample::unit::sample::shared::addsToBase ... ok
test sample::unit::sample::shared::nested::seesBoth ... ok
test sample::unit::sample::skipped::isSkipped ... skipped: its suite sample::unit::sample::skipped skipped
test sample::unit::sample::skipsWithAReason ... skipped: needs a database
test sample::unit::sample::spinsForever ... TIMEOUT

--- suite sample::unit::sample::brokenSetup (setup)
error: ParseError.Empty
 --> src/main_test.to:56:17

--- sample::unit::sample::failsAnAssertion
panic[P0007]: assertion-failed: twice(2) is 4
 --> src/main_test.to:11:5
stdout:
about to fail

--- sample::unit::sample::returnsAnError
error: ParseError.Empty
 --> src/main_test.to:15:18

--- suite sample::unit::sample::skipped (setup)
skipped: no network
 --> src/main_test.to:64:5

--- sample::unit::sample::spinsForever
timed out after 1s

FAILED. 6 passed; 3 failed; 2 skipped; 1 blocked
";

#[test]
fn runs_and_reports() {
    let (stdout, stderr, status) = tondo(&sample(), &["test", "--timeout", "1"]);
    assert_eq!(untimed(&stdout), REPORT, "stderr:\n{stderr}");
    assert_eq!(status, 1);
}

#[test]
fn shows_what_passing_tests_wrote() {
    let (stdout, _, status) = tondo(&sample(), &["test", "--show-output", "--filter", "logsAndTags"]);
    let expected = "test sample::unit::sample::logsAndTags ... ok\n\n--- sample::unit::sample::logsAndTags\nstdout:\nlogged\nlogs:\nfirst\ntags: area=parse\n\nok. 1 passed; 0 failed; 0 skipped; 0 blocked\n";
    assert_eq!(untimed(&stdout), expected);
    assert_eq!(status, 0);
}

#[test]
fn selects_and_lists() {
    let (stdout, _, status) = tondo(&sample(), &["test", "--list", "--exact", "sample::unit::sample::shared"]);
    let listed = "sample::unit::sample::shared (suite)\nsample::unit::sample::shared::addsToBase\nsample::unit::sample::shared::nested (suite)\nsample::unit::sample::shared::nested::seesBoth\n";
    assert_eq!((stdout.as_str(), status), (listed, 0));
    let (stdout, _, status) = tondo(&sample(), &["test", "--filter", "nested"]);
    let report =
        "test sample::unit::sample::shared::nested::seesBoth ... ok\nok. 1 passed; 0 failed; 0 skipped; 0 blocked\n";
    assert_eq!((untimed(&stdout).as_str(), status), (report, 0));
    let (_, stderr, status) = tondo(&sample(), &["test", "--filter", "missing"]);
    assert_eq!((stderr.as_str(), status), ("tondo: no test matches --filter \"missing\"\n", 1));
    let (_, _, status) = tondo(&sample(), &["test", "--filter", "missing", "--allow-empty"]);
    assert_eq!(status, 0);
    let (_, _, status) = tondo(&sample(), &["test", "--filter", "skipsWithAReason", "--deny-skips"]);
    assert_eq!(status, 1);
}

/// A panic in a task a test started still runs the test's cleanups and its
/// suite's teardown; the next test runs in a new worker.
#[test]
fn tears_down_after_a_panic_in_a_task() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("testing-task-panic");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"tasks\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "fn main() {}\n").unwrap();
    let tests = "\
import std.console

fn boom() {
    panic(\"in a task\")
}

suite service {
    defer console.println(\"stop service\")

    test panicsInATask {
        defer console.println(\"test cleanup\")
        let job = spawn boom()
        job.wait()
    }

    test runsAfter {}
}
";
    std::fs::write(dir.join("src/main_test.to"), tests).unwrap();
    let (stdout, _, status) = tondo(&dir, &["test"]);
    let expected = "\
test tasks::unit::tasks::service::panicsInATask ... FAILED
test tasks::unit::tasks::service::runsAfter ... ok

--- tasks::unit::tasks::service::panicsInATask
panic[P0008]: explicit-panic: in a task
 --> src/main_test.to:4:5
stdout:
test cleanup
stop service

FAILED. 1 passed; 1 failed; 0 skipped; 0 blocked
";
    assert_eq!((untimed(&stdout).as_str(), status), (expected, 1));
}

#[test]
fn rejects_wrong_tests() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("testing-errors");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"wrong\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "fn main() {}\n").unwrap();
    let tests = "\
test same {}

test same {}

suite empty {}

suite captures {
    var counter = 0
    let fine = 1

    test usesVar {
        assert(counter + fine == 1)
    }
}

suite returns {
    return

    test unreachable {}
}
";
    std::fs::write(dir.join("src/main_test.to"), tests).unwrap();
    let (_, stderr, status) = tondo(&dir, &["test"]);
    let codes: Vec<&str> = stderr.lines().filter_map(|l| l.strip_prefix("error[")?.split(']').next()).collect();
    assert_eq!(codes, ["E2002", "E2004", "E2005", "E1205"], "{stderr}");
    assert_eq!(status, 1);
    // A test in a production file, and `std.testing` there.
    std::fs::write(dir.join("src/main.to"), "fn main() {}\n\ntest misplaced {}\n").unwrap();
    let (_, stderr, _) = tondo(&dir, &["test"]);
    assert!(stderr.contains("error[E2001]"), "{stderr}");
    std::fs::write(dir.join("src/main.to"), "import std.testing\n\nfn main() {}\n").unwrap();
    let (_, stderr, _) = tondo(&dir, &["check"]);
    assert!(stderr.contains("error[E2003]"), "{stderr}");
}

/// A project in the target directory, a repository of its own, with the
/// given test file.
fn project(name: &str, tests: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(dir.join("src/main.to"), "fn main() {}\n").unwrap();
    std::fs::write(dir.join("src/main_test.to"), tests).unwrap();
    dir
}

#[test]
fn retries_repeats_and_owners() {
    let tests = "\
import std.fs

// Fails while the file `flag` is missing, and makes it.
test flaky {
    if not fs.exists(\"flag\") {
        fs.writeText(\"flag\", \"\")?
        assert(false, \"first run\")
    }
}

test steady {}
";
    let dir = project("testing-retries", tests);
    std::fs::create_dir_all(dir.join(".github")).unwrap();
    std::fs::write(dir.join(".github/CODEOWNERS"), "# owners\n* @all\n*_test.to @testers\n").unwrap();
    let (stdout, _, status) = tondo(&dir, &["test", "--retry", "1"]);
    let expected = "\
test app::unit::app::flaky ... FAILED
test app::unit::app::steady ... ok

retry round 1:
test app::unit::app::flaky ... ok

--- app::unit::app::flaky (flaky: it passed after failing)
owners: @testers
attempt 1: panic[P0007]: assertion-failed: first run
 --> src/main_test.to:7:9
attempt 2, retry round 1: passed

FAILED. 1 passed; 1 flaky; 0 failed; 0 skipped; 0 blocked
";
    assert_eq!((untimed(&stdout).as_str(), status), (expected, 1));
    std::fs::remove_file(dir.join("flag")).unwrap();
    let (_, _, status) = tondo(&dir, &["test", "--retry", "1", "--allow-flaky"]);
    assert_eq!(status, 0);
    // Repeating finds it too, and passes only when every run passes.
    std::fs::remove_file(dir.join("flag")).unwrap();
    let (stdout, _, status) = tondo(&dir, &["test", "--repeat", "2", "--filter", "flaky"]);
    assert!(stdout.contains("\niteration 2:\ntest app::unit::app::flaky ... ok\n"), "{stdout}");
    assert_eq!(status, 1);
    let (_, _, status) = tondo(&dir, &["test", "--repeat", "3", "--filter", "steady"]);
    assert_eq!(status, 0);
    let (stdout, _, _) = tondo(&dir, &["test", "--list", "--codeowners", "none"]);
    assert_eq!(stdout, "app::unit::app::flaky\napp::unit::app::steady\n");
    let (stdout, _, _) = tondo(&dir, &["test", "--list"]);
    assert_eq!(stdout, "app::unit::app::flaky  @testers\napp::unit::app::steady  @testers\n");
}

#[test]
fn shards_globs_and_orders() {
    let mut tests = String::new();
    for name in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"] {
        tests.push_str(&format!("test {name} {{}}\n\n"));
    }
    tests.push_str("suite group {\n    test inner {}\n}\n");
    let dir = project("testing-shards", &tests);
    let ran = |args: &[&str]| -> Vec<String> {
        let (stdout, _, _) = tondo(&dir, &[&["test"], args].concat());
        stdout
            .lines()
            .filter_map(|l| l.strip_prefix("test "))
            .map(|l| l.split(' ').next().unwrap().to_string())
            .collect()
    };
    // The shards split the tests between them.
    let mut all: Vec<String> = (1..=3).flat_map(|i| ran(&["--shard", &format!("{i}/3")])).collect();
    all.sort();
    assert_eq!(all, ran(&[]));
    assert_eq!(
        ran(&["--glob", "app::unit::app::*ta"]),
        ["app::unit::app::beta", "app::unit::app::delta", "app::unit::app::zeta"]
    );
    assert_eq!(ran(&["--glob", "**::group"]), ["app::unit::app::group::inner"]);
    // A seed gives its order every time; lines stay in canonical order.
    let (first, _, _) = tondo(&dir, &["test", "--order", "random", "--seed", "5eed"]);
    let (again, _, _) = tondo(&dir, &["test", "--order", "random", "--seed", "5eed"]);
    assert!(first.starts_with("seed 0000000000005eed\n"));
    assert_eq!(untimed(&first), untimed(&again));
    for (args, message) in [
        (&["--glob", "a::**::**"][..], "two `**` components follow each other"),
        (&["--shard", "0/3"], "--shard takes index/count"),
        (&["--seed", "5eed"], "--seed goes with --order random"),
        (&["--repeat", "2", "--retry", "1"], "it does not go with --retry"),
        (&["--filter", "a", "--exact", "b"], "give only one of"),
    ] {
        let (_, stderr, status) = tondo(&dir, &[&["test"], args].concat());
        assert!(stderr.contains(message) && status == 2, "{args:?}: {stderr}");
    }
}
