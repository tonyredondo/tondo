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
 --> src/main_test.to:55:7

--- sample::unit::sample::failsAnAssertion
panic[P0007]: assertion-failed: twice(2) is 4
 --> src/main_test.to:11:5
stdout:
about to fail

--- sample::unit::sample::returnsAnError
error: ParseError.Empty
 --> src/main_test.to:14:6

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
