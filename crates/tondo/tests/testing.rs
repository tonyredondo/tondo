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
  condition: twice(2) == 5
  left:  4
  right: 5
 --> src/main_test.to:11:5
stack:
  test failsAnAssertion  src/main_test.to:11:5
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
    // `--exact` again chooses more, in one run; each must name something.
    let (stdout, _, status) = tondo(
        &sample(),
        &[
            "test",
            "--exact",
            "sample::unit::sample::doublesPrivately",
            "--exact",
            "sample::unit::sample::shared::nested",
        ],
    );
    let report = "test sample::unit::sample::doublesPrivately ... ok\ntest sample::unit::sample::shared::nested::seesBoth ... ok\nok. 2 passed; 0 failed; 0 skipped; 0 blocked\n";
    assert_eq!((untimed(&stdout).as_str(), status), (report, 0));
    let (stdout, _, _) = tondo(
        &sample(),
        &[
            "test",
            "--list",
            "--test-format",
            "json",
            "--exact",
            "sample::unit::sample::doublesPrivately",
            "--exact",
            "sample::unit::sample::logsAndTags",
        ],
    );
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        list["selection"],
        serde_json::json!({ "kind": "exact", "value": ["sample::unit::sample::doublesPrivately", "sample::unit::sample::logsAndTags"] })
    );
    let (_, stderr, status) =
        tondo(&sample(), &["test", "--exact", "sample::unit::sample::doublesPrivately", "--exact", "nothing"]);
    assert_eq!(
        (stderr.as_str(), status),
        ("tondo: no suite or test is \"nothing\"; `tondo test --list` lists them\n", 1)
    );
    let (_, stderr, status) = tondo(&sample(), &["test", "--exact", "a", "--filter", "b"]);
    assert_eq!(status, 2, "{stderr}");
}

/// Listing analyzes the tests without building them: a library that
/// cannot be linked does not keep the list from coming.
#[test]
fn lists_without_building() {
    let dir = project("list-unlinked", "test works {\n    assert(1 == 1)\n}\n");
    std::fs::write(
        dir.join("tondo.toml"),
        "[package]\nname = \"app\"\n\n[native]\nlibraries = [\"tondo_missing_library\"]\n",
    )
    .unwrap();
    let (stdout, stderr, status) = tondo(&dir, &["test", "--list"]);
    assert_eq!((stdout.as_str(), status), ("app::unit::app::works\n", 0), "{stderr}");
    // Running them fails: on Linux at the link, and on macOS, whose
    // libraries are not files to look for, when the tests start.
    let (_, _, status) = tondo(&dir, &["test"]);
    assert_ne!(status, 0);
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
stack:
  boom  src/main_test.to:4:5
in a task spawned at:
  test panicsInATask  src/main_test.to:12:19
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
  condition: false
 --> src/main_test.to:7:9
stack:
  test flaky  src/main_test.to:7:9
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

#[test]
fn writes_json_and_junit_reports() {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("testing-reports");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let (json, xml) = (out.join("report.json"), out.join("report.xml"));
    let reports = [format!("json={}", json.display()), format!("junit={}", xml.display())];
    let args = ["test", "--timeout", "1", "--test-format", "json", "--report", &reports[0], "--report", &reports[1]];
    let (stdout, _, status) = tondo(&sample(), &args);
    assert_eq!(status, 1);
    // stdout and the file hold the same bytes.
    assert_eq!(std::fs::read_to_string(&json).unwrap(), stdout);
    for part in [
        "{\"format\":\"tondo-test-report-0.1/1\",\"edition\":\"0.1\",",
        "\"compiled\":true,\"selection\":{\"kind\":\"all\",\"value\":null},",
        "\"stack\":[{\"function\":\"test failsAnAssertion\",\"source\":{\"file\":\"src/main_test.to\",\"line\":11,\"column\":5},\"task\":0,\"times\":1}]",
        "\"failure\":{\"kind\":\"error\",\"code\":null,\"error_type\":\"ParseError\",\"message\":\"ParseError.Empty\",\"source\":{\"file\":\"src/main_test.to\",\"line\":15,\"column\":18},\"comparison\":null,\"stack\":[]}",
        "\"comparison\":{\"left\":\"4\",\"right\":\"5\"},\"stack\":[{\"function\":\"test failsAnAssertion\"",
        "\"blocked_by\":{\"id\":\"sample::unit::sample::brokenSetup\",\"attempt\":1}",
        "\"skip\":{\"reason\":\"needs a database\",\"source\":{\"file\":\"src/main_test.to\",\"line\":20,\"column\":5}}",
        "\"tags\":{\"area\":\"parse\"}",
        "\"summary\":{\"selected\":12,\"executed\":10,\"passed\":6,\"flaky_passed\":0,\"skipped\":1,\"blocked_setup\":1,\"blocked_skip\":1,\"failed_error\":1,\"failed_panic\":1,\"resource_limit\":0,\"timeout\":1,\"crashed\":0,",
    ] {
        assert!(stdout.contains(part), "missing {part} in {stdout}");
    }
    assert!(stdout.ends_with("}\n") && !stdout.contains("\ntest "));
    let xml = std::fs::read_to_string(&xml).unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"13\" failures=\"3\" errors=\"1\" skipped=\"3\" time=\""));
    for part in [
        "<testsuite name=\"sample::unit::sample::brokenSetup\" tests=\"2\" failures=\"1\" errors=\"0\" skipped=\"1\"",
        "<testcase name=\"@setup\" classname=\"sample::unit::sample::brokenSetup\"",
        "<failure type=\"P0007\" message=\"assertion-failed: twice(2) is 4\">",
        "<error type=\"timeout\" message=\"timed out after 1s\">",
        "<skipped message=\"needs a database\"/>",
        "<system-out>about to fail\n</system-out>",
    ] {
        assert!(xml.contains(part), "missing {part} in {xml}");
    }
    // The listing in JSON.
    let (stdout, _, _) = tondo(&sample(), &["test", "--list", "--test-format", "json", "--filter", "nested"]);
    assert!(stdout.starts_with("{\"format\":\"tondo-test-list-0.1/1\""));
    assert!(stdout.contains("\"execution_plan\":[\"sample::unit::sample::shared::nested::seesBoth\"]"));
}

#[test]
fn attaches_and_checks_snapshots() {
    let tests = "\
import std.testing

fn render(name: String): String {
    \"Hello, {name}!\\nBye.\"
}

test rendersGreeting {
    testing.attach(\"greeting.txt\", \"text/plain\", render(\"Ada\").bytes())
    testing.snapshot(\"greeting\", render(\"Ada\"))
}

test badMedia {
    testing.attach(\"x\", \"not a type\", [])
}
";
    let dir = project("testing-snapshots", tests);
    let (stdout, _, status) = tondo(&dir, &["test", "--filter", "rendersGreeting"]);
    assert!(stdout.contains("panic[P2007]: test-snapshot-mismatch: there is no snapshot \"greeting\""), "{stdout}");
    assert_eq!(status, 1);
    let (_, _, status) = tondo(&dir, &["test", "--filter", "rendersGreeting", "--update-snapshots"]);
    assert_eq!(status, 0);
    let store = std::fs::read_to_string(dir.join("snapshots/unit/app/rendersGreeting/greeting.snap")).unwrap();
    assert_eq!(store, "Hello, Ada!\nBye.");
    let (_, _, status) = tondo(&dir, &["test", "--filter", "rendersGreeting"]);
    assert_eq!(status, 0);
    let changed = tests.replace("Bye.", "Goodbye.");
    std::fs::write(dir.join("src/main_test.to"), changed).unwrap();
    let (stdout, _, status) = tondo(&dir, &["test", "--filter", "rendersGreeting", "--test-format", "json"]);
    assert_eq!(status, 1);
    assert!(stdout.contains("\"snapshots\":[{\"name\":\"greeting\",\"status\":\"mismatched\","), "{stdout}");
    assert!(
        stdout.contains("\"artifacts\":[{\"name\":\"greeting.txt\",\"media_type\":\"text/plain\",\"size\":\"20\",")
    );
    let (stdout, _, _) = tondo(&dir, &["test", "--filter", "rendersGreeting"]);
    assert!(stdout.contains("snapshot greeting: mismatched\n  line 2, expected: Bye.\n  line 2, found:    Goodbye.\n"));
    let manifest = std::fs::read_to_string(dir.join("target/test-artifacts/manifest.json")).unwrap();
    assert!(manifest.starts_with("{\"format\":\"tondo-test-artifacts-0.1/1\",\"objects\":[{\"sha256\":"));
    let (stdout, _, _) = tondo(&dir, &["test", "--filter", "badMedia"]);
    assert!(stdout.contains("panic[P2006]: test-artifact-conflict: \"not a type\" is not a media type"));
    let (stderr, status) = {
        let (_, stderr, status) = tondo(&dir, &["test", "--update-snapshots", "--jobs", "2"]);
        (stderr, status)
    };
    assert!(stderr.contains("--update-snapshots runs with --jobs 1") && status == 2);
}

#[test]
fn snapshots_values_as_json() {
    let tests = r#"
import std.testing

type Item = { name: String, price: Int }
type Order = { id: String, createdAt: String, items: Array[Item], total: Int }

fn order(id: String): Order {
    Order { id, createdAt: "2026-10-22T18:00:00Z", items: [Item { name: "book", price: 12 }], total: 12 }
}

test createsOrder {
    testing.snapshot("order", order("3fa85f64"), ignore: ["createdAt"], matches: ["id": r"[0-9a-f]{8}"])
    testing.snapshot("log", "took 12ms, then 3ms", replace: [r"\d+ms": "[time]"])
}

test badId {
    testing.snapshot("order", order("nope"), matches: ["id": r"[0-9a-f]{8}"])
}
"#;
    let dir = project("testing-json-snapshots", tests);
    let (_, _, status) = tondo(&dir, &["test", "--filter", "createsOrder", "--update-snapshots"]);
    assert_eq!(status, 0);
    let read = |name: &str| std::fs::read_to_string(dir.join("snapshots/unit/app/createsOrder").join(name)).unwrap();
    assert_eq!(
        read("order.snap"),
        "{\n  \"createdAt\": \"[ignored]\",\n  \"id\": \"[matches [0-9a-f]{8}]\",\n  \"items\": [\n    {\n      \"name\": \"book\",\n      \"price\": 12\n    }\n  ],\n  \"total\": 12\n}"
    );
    assert_eq!(read("log.snap"), "took [time], then [time]");
    let (_, _, status) = tondo(&dir, &["test", "--filter", "createsOrder"]);
    assert_eq!(status, 0);
    let (stdout, _, status) = tondo(&dir, &["test", "--filter", "badId"]);
    assert_eq!(status, 1);
    let message = r#"panic[P2007]: test-snapshot-mismatch: in the snapshot "order", the value at "id" is "nope", which does not match the pattern "[0-9a-f]{8}""#;
    assert!(stdout.contains(message), "{stdout}");
    // `ignore` and `matches` look into JSON, which a `String` is not.
    std::fs::write(
        dir.join("src/main_test.to"),
        tests.replace(r#""took 12ms, then 3ms","#, r#""took 12ms", ignore: ["x"],"#),
    )
    .unwrap();
    let (stdout, stderr, status) = tondo(&dir, &["test", "--filter", "createsOrder"]);
    assert!(stderr.contains("error[E1102]: `ignore` and `matches` look into the JSON of a value"), "{stdout}{stderr}");
    assert_eq!(status, 1);
}

#[test]
fn runs_on_a_virtual_clock() {
    let tests = "\
import std.console
import std.testing
import std.time

fn retryAfter(delay: time.Duration): Int {
    time.sleep(delay)
    time.sleep(delay)
    2
}

// Ten seconds of backoff, without waiting for them.
test backsOff {
    let started = time.now()
    testing.withVirtualTime((clock) {
        let before = time.now()
        let job = spawn retryAfter(time.seconds(5))
        clock.settle()
        clock.advance(time.seconds(5))
        assert(job.wait() == 2)
        console.println(\"virtual {before.elapsed()}\")
    })
    assert(started.elapsed().isLessThan(time.seconds(1)), \"it waited\")
}

test waitsForNothing {
    let (sender, receiver) = channel[Int]()
    testing.withVirtualTime((clock) {
        let value = receiver.receive()
    })
}

test nests {
    testing.withVirtualTime((outer) {
        testing.withVirtualTime((inner) {})
    })
}
";
    let dir = project("testing-virtual", tests);
    let (stdout, _, status) = tondo(&dir, &["test", "--show-output", "--timeout", "10"]);
    assert_eq!(status, 1);
    for part in [
        "test app::unit::app::backsOff ... ok\n",
        "stdout:\nvirtual 10s\nvirtual clock 1: at 10s of virtual time, moved 1 times by itself and 1 by advance, settled 1 times\n",
        "panic[P2003]: test-virtual-time-deadlock",
        "panic[P2004]: overlapping-test-virtual-time",
    ] {
        assert!(stdout.contains(part), "missing {part} in {stdout}");
    }
    let (stdout, _, _) = tondo(&dir, &["test", "--filter", "backsOff", "--test-format", "json"]);
    let clock = "\"virtual_time\":[{\"index\":1,\"elapsed_ns\":\"10000000000\",\"automatic_advances\":1,\"explicit_advances\":1,\"settles\":1}]";
    assert!(stdout.contains(clock), "{stdout}");
}

#[test]
fn explores_orders_of_tasks() {
    let tests = "\
fn worker(id: Int, out: Sender[Int]) {
    out.send(id)
}

test firstArrivesFirst {
    let (out, inbox) = channel[Int]()
    spawn detached worker(1, out.copy())
    spawn detached worker(2, out.copy())
    assert(inbox.receive() == some(1))
}
";
    let dir = project("explores-orders", tests);
    let (stdout, stderr, status) = tondo(&dir, &["test", "--schedules", "16", "--seed", "1"]);
    assert_eq!(status, 1, "{stdout}{stderr}");
    assert!(stdout.starts_with("16 orders of the tasks from seed 0000000000000001\n\norder 1, seed 1:\n"), "{stdout}");
    let again = stdout.lines().find_map(|line| line.strip_prefix("again in that order: tondo ")).expect(&stdout);
    let args: Vec<&str> = again.split(' ').collect();
    // The order that failed fails again, every time.
    for _ in 0..3 {
        let (stdout, _, status) = tondo(&dir, &args);
        assert_eq!(status, 1, "{stdout}");
    }
    let (stdout, stderr, status) = tondo(&dir, &["test", "--schedules", "2", "--repeat", "2"]);
    assert_eq!(status, 2, "{stdout}{stderr}");
    assert!(stderr.contains("--schedules runs everything once per order; it does not go with --repeat"), "{stderr}");
}

#[test]
fn fails_tests_that_leave_tasks_running() {
    let tests = "\
// It keeps a sender of its own channel, which so never closes.
fn listen(inbox: Receiver[Int], keep: Sender[Int]) {
    for request in inbox {
        _ = request
    }
    keep.close()
}

test leavesAListener {
    let (requests, inbox) = channel[Int]()
    spawn detached listen(inbox, requests.copy())
    requests.send(1)
}

test waitsForItsTasks {
    let (requests, inbox) = channel[Int]()
    let job = spawn listen(inbox, requests.copy())
    requests.close()
    job.wait()
}
";
    let dir = project("leaves-tasks", tests);
    let (stdout, stderr, status) = tondo(&dir, &["test"]);
    assert_eq!(status, 1, "{stdout}{stderr}");
    let report = "\
--- app::unit::app::leavesAListener
error: the test left 1 task running; wait for it, cancel it, or close what it waits on
a task waits at:
  listen  src/main_test.to:3:20
in a task spawned at:
  test leavesAListener  src/main_test.to:11:5
";
    assert!(stdout.contains(report), "{stdout}");
    assert!(stdout.contains("test app::unit::app::waitsForItsTasks ... ok"), "{stdout}");
}

#[test]
fn dev_dependencies_are_for_tests() {
    let tests = "import helpers\n\ntest usesHelpers {\n    assert(helpers.twice(3) == 6)\n}\n";
    let dir = project("testing-dev-dependencies", tests);
    let helpers = dir.join("helpers");
    std::fs::create_dir_all(&helpers).unwrap();
    std::fs::write(helpers.join("main.to"), "pub fn twice(n: Int): Int {\n    n * 2\n}\n").unwrap();
    let manifest = "[package]\nname = \"app\"\n\n[dev-dependencies]\nhelpers = { path = \"helpers\" }\n";
    std::fs::write(dir.join("tondo.toml"), manifest).unwrap();
    let (stdout, _, status) = tondo(&dir, &["test"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.contains("test app::unit::app::usesHelpers ... ok"), "{stdout}");
    // The program does not see it.
    std::fs::write(dir.join("src/main.to"), "import helpers\n\nfn main() {}\n").unwrap();
    let (_, stderr, status) = tondo(&dir, &["build"]);
    assert_ne!(status, 0);
    assert!(stderr.contains("error[E1008]: `helpers` is a dev-dependency, which only tests import"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn an_interrupted_run_cancels_its_tests() {
    let tests = "\
import std.fs
import std.time

test early {
    assert(true)
}

test waits {
    defer {
        _ = fs.writeText(\"cleaned\", \"yes\")
    }
    _ = fs.writeText(\"started\", \"yes\")
    time.sleep(time.seconds(60))
}
";
    let dir = project("interrupted", tests);
    let child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["test", "--jobs", "1", "--report", "json=report.json"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !dir.join("started").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(4), "{stdout}{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.starts_with("test app::unit::app::early ... ok\n"), "{stdout}");
    assert!(stdout.contains("interrupted: 1 of 2 tests ran; no report was written"), "{stdout}");
    assert!(dir.join("cleaned").exists(), "the test's defer did not run");
    assert!(!dir.join("report.json").exists());
}

/// A package with no `main`, as one that others depend on, is checked
/// and tested (testing 6.3); only running it needs an entry point.
#[test]
fn a_package_without_main() {
    let dir = project("no_main", "test doubles {\n    assert(twice(2) == 4)\n}\n");
    std::fs::write(dir.join("src/main.to"), "pub fn twice(n: Int): Int {\n    n * 2\n}\n").unwrap();
    let (stdout, stderr, status) = tondo(&dir, &["test"]);
    assert_eq!(status, 0, "{stdout}{stderr}");
    assert!(stdout.contains("app::unit::app::doubles ... ok"), "{stdout}");
    let (_, stderr, status) = tondo(&dir, &["check"]);
    assert_eq!((status, stderr.as_str()), (0, ""));
    let (_, stderr, status) = tondo(&dir, &["run"]);
    assert!(status != 0 && stderr.contains("error[E1806]: nothing to run"), "{stderr}");
}

/// `--test-format events`: a line of JSON as each pass begins and each
/// suite or test starts and ends, retries included, then the report.
#[test]
fn reports_events_as_tests_run() {
    let tests = "\
import std.console
import std.testing

test passes {
    console.println(\"said\")
}

test fails {
    assert(1 == 2, \"one is not two\")
}

suite group {
    test skips {
        testing.skip(\"later\")
    }
}
";
    let dir = project("events", tests);
    let (stdout, stderr, status) = tondo(&dir, &["test", "--test-format", "events", "--retry", "1"]);
    assert_eq!(status, 1, "{stderr}");
    let events: Vec<serde_json::Value> = stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let short = |id: &serde_json::Value| id.as_str().map(|id| id.rsplit("::").next().unwrap().to_string());
    let seen: Vec<String> = events
        .iter()
        .map(|e| {
            let id = short(&e["id"]);
            let what = [e["event"].as_str(), id.as_deref(), e["status"].as_str()];
            format!("{} r{}", what.iter().flatten().copied().collect::<Vec<_>>().join(" "), e["round"])
        })
        .collect();
    assert_eq!(
        seen,
        [
            "pass r0",
            "start fails r0",
            "end fails failed-panic r0",
            "start group r0",
            "start skips r0",
            "end skips skipped r0",
            "end group passed r0",
            "start passes r0",
            "output passes r0",
            "end passes passed r0",
            "pass r1",
            "start fails r1",
            "end fails failed-panic r1",
            "report rnull",
        ]
    );
    // What a test writes comes as it writes it, and again with its end.
    assert_eq!(events[8]["stdout"]["data"], "said\n");
    assert_eq!(events[0]["format"], "tondo-test-events-0.1/1");
    assert_eq!(events[0]["tests"].as_array().unwrap().len(), 3);
    assert_eq!(
        events[2]["failure"]["message"],
        "assertion-failed: one is not two\n  condition: 1 == 2\n  left:  1\n  right: 2"
    );
    assert_eq!(events[2]["failure"]["source"]["line"], 9);
    assert_eq!(events[9]["stdout"]["data"], "said\n");
    // The two sides of the comparison, for a diff.
    assert_eq!(events[2]["failure"]["comparison"], serde_json::json!({ "left": "1", "right": "2" }));
    let report = &events.last().unwrap()["report"];
    assert_eq!((&report["format"], &report["summary"]["failed_panic"]), (&"tondo-test-report-0.1/1".into(), &1.into()));

    // What does not compile has only the report, which says so.
    std::fs::write(dir.join("src/main_test.to"), "test broken {\n    missing()\n}\n").unwrap();
    let (stdout, _, status) = tondo(&dir, &["test", "--test-format", "events"]);
    assert_eq!(status, 1);
    let event: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!((&event["event"], &event["report"]["compiled"]), (&"report".into(), &false.into()));
}

/// `--watch` runs the tests, then after each change those it may affect,
/// until it is interrupted: here, a change to a module runs the tests of
/// what imports it and not the others.
#[test]
fn watches_and_runs_what_a_change_affects() {
    use std::io::BufRead;
    let dir = project("watch", "test standsAlone {\n    assert(2 == 2)\n}\n");
    std::fs::create_dir_all(dir.join("src/shapes")).unwrap();
    std::fs::write(dir.join("src/shapes/shapes.to"), "pub fn sides(): Int {\n    3\n}\n").unwrap();
    std::fs::write(dir.join("src/shapes/shapes_test.to"), "test hasSides {\n    assert(sides() == 3)\n}\n").unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(
        dir.join("tests/api.to"),
        "import app.shapes\n\ntest usesShapes {\n    assert(shapes.sides() == 3)\n}\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["test", "--watch", "--test-format", "events"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let (sender, lines) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let _ = sender.send(line.unwrap());
        }
    });
    // The tests of each run, by the `pass` that starts it.
    let next_run = || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut tests = Vec::new();
        loop {
            let line = lines.recv_timeout(deadline - std::time::Instant::now()).expect("an event");
            let event: serde_json::Value = serde_json::from_str(&line).unwrap();
            match event["event"].as_str().unwrap() {
                "pass" => {
                    tests = event["tests"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_string()).collect()
                }
                "report" => return tests,
                _ => {}
            }
        }
    };
    let all = ["app::integration::api::usesShapes", "app::unit::app::standsAlone", "app::unit::shapes::hasSides"];
    assert_eq!(next_run(), all);
    // Stamps change at least each second on some filesystems.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(dir.join("src/shapes/shapes.to"), "pub fn sides(): Int {\n    1 + 2\n}\n").unwrap();
    assert_eq!(next_run(), ["app::integration::api::usesShapes", "app::unit::shapes::hasSides"]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(dir.join("src/main_test.to"), "test standsAlone {\n    assert(3 == 3)\n}\n").unwrap();
    assert_eq!(next_run(), ["app::unit::app::standsAlone"]);
    // The manifest changed: everything runs.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n\n").unwrap();
    assert_eq!(next_run(), all);
    unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    assert_eq!(child.wait().unwrap().code(), Some(4));
}

const COVERED: &str = "import std.console

pub enum Shape {
    Circle(Float)
    Square(Float)
    Point
}

pub fn area(shape: Shape): Float {
    match shape {
        Shape.Circle(r) => 3.0 * r * r
        Shape.Square(s) => s * s
        Shape.Point => 0.0
    }
}

pub fn sign(n: Int): Int {
    if n > 0 {
        return 1
    } else {
        return -1
    }
}

pub fn both(a: Bool, b: Bool): Bool {
    a and b
}

pub fn unused(): Int {
    42
}

fn main() {
    console.println(\"{area(Shape.Point)}\")
}
";

/// Coverage: the lines, functions and ways of branches of the project's
/// code that the tests ran, as JSON, LCOV and a summary; what no test
/// calls counts as not run, and the tests themselves do not count.
#[test]
fn measures_coverage() {
    let tests = "\
test areas {
    assert(area(Shape.Circle(1.0)) == 3.0)
    assert(area(Shape.Square(2.0)) == 4.0)
}

test signs {
    assert(sign(5) == 1)
}

test logic {
    assert(both(false, true) == false)
}
";
    let dir = project("coverage", tests);
    std::fs::write(dir.join("src/main.to"), COVERED).unwrap();
    let (json, lcov) = (dir.join("coverage.json"), dir.join("lcov.info"));
    let reports = [format!("coverage={}", json.display()), format!("lcov={}", lcov.display())];
    let (stdout, stderr, status) =
        tondo(&dir, &["test", "--coverage", "--jobs", "2", "--report", &reports[0], "--report", &reports[1]]);
    assert_eq!(status, 0, "{stderr}");
    assert!(
        stdout.ends_with(
            "\ncoverage:\n  src/main.to  lines 60.0% (6/10)      functions 60.0% (3/5)     branches 57.1% (4/7)\n  total        lines 60.0% (6/10)      functions 60.0% (3/5)     branches 57.1% (4/7)\n"
        ),
        "{stdout}"
    );
    let covered: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(covered["format"], "tondo-coverage-0.1/1");
    assert_eq!(covered["files"].as_array().unwrap().len(), 1, "only the project's code counts: {covered}");
    let file = &covered["files"][0];
    assert_eq!(file["file"], "src/main.to");
    let lines: Vec<(u64, u64)> = file["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["line"].as_u64().unwrap(), l["count"].as_u64().unwrap()))
        .collect();
    // Each line counts the times a statement written on it started: the
    // `match` once a call, each arm once it ran.
    assert_eq!(lines, [(10, 2), (11, 1), (12, 1), (13, 0), (18, 1), (19, 1), (21, 0), (26, 1), (30, 0), (34, 0)]);
    let functions: Vec<(&str, u64, u64)> = file["functions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap(), f["line"].as_u64().unwrap(), f["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(functions, [("area", 9, 2), ("sign", 17, 1), ("both", 25, 1), ("unused", 29, 0), ("main", 33, 0)]);
    let ways: Vec<(u64, &str, u64)> = file["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["branch"]["line"].as_u64().unwrap(), b["label"].as_str().unwrap(), b["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        ways,
        [
            (10, "Shape.Circle(r)", 1),
            (10, "Shape.Square(s)", 1),
            (10, "Shape.Point", 0),
            (18, "then", 1),
            (18, "else", 0),
            (26, "short-circuited", 1),
            (26, "evaluated", 0),
        ]
    );
    assert_eq!(covered["summary"]["branches"], serde_json::json!({ "found": 7, "hit": 4 }));
    let lcov = std::fs::read_to_string(&lcov).unwrap();
    for part in [
        "TN:\nSF:src/main.to\nFN:9,area\n",
        "FNDA:0,unused\n",
        "BRDA:10,0,2,0\n",
        "DA:13,0\n",
        "LF:10\nLH:6\nend_of_record\n",
    ] {
        assert!(lcov.contains(part), "missing {part:?} in {lcov}");
    }
    // A loop's head counts each check, its body each turn.
    let looping = project("coverage-loops", "test sums {\n    assert(total([1, 2, 3]) == 8)\n}\n");
    let source = "pub fn total(values: Array[Int]): Int {\n    var sum = 0\n    for value in values {\n        sum = sum + value\n    }\n    var n = 0\n    for n < 2 {\n        n = n + 1\n    }\n    sum + n\n}\n\nfn main() {}\n";
    std::fs::write(looping.join("src/main.to"), source).unwrap();
    let report = looping.join("coverage.json");
    let (_, stderr, status) = tondo(&looping, &["test", "--report", &format!("coverage={}", report.display())]);
    assert_eq!(status, 0, "{stderr}");
    let covered: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    let lines: Vec<(u64, u64)> = covered["files"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["line"].as_u64().unwrap(), l["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(lines, [(2, 1), (3, 4), (4, 3), (6, 1), (7, 3), (8, 2), (10, 1)]);
    // Without coverage the build counts nothing, and says nothing of it.
    let (stdout, _, _) = tondo(&dir, &["test"]);
    assert!(!stdout.contains("coverage"), "{stdout}");
}

#[test]
fn measures_benchmarks() {
    let tests = "\
import std.console
import std.testing

fn fib(n: Int): Int {
    if n < 2 {
        return n
    }
    fib(n - 1) + fib(n - 2)
}

fn parse(text: String): Int ! String {
    if text == \"\" {
        fail \"empty\"
    }
    text.length()
}

test fibonacci {
    for n in [5, 15] {
        testing.bench(\"fib {n}\", () { fib(n) })
    }
}

test once {
    testing.bench(\"print\", () { console.println(\"ran\") })
}

test joins {
    testing.bench(\"join\", () { \"{[1, 2, 3]}\" })
}

test fails {
    testing.bench(\"parse\", () { parse(\"\")? })?
}

test twice {
    testing.bench(\"same\", () { 1 })
    testing.bench(\"same\", () { 2 })
}

test plain {
    assert(fib(3) == 2)
}
";
    let dir = project("benchmarks", tests);
    // A plain run runs each body once.
    let (stdout, _, status) = tondo(&dir, &["test", "--show-output"]);
    assert_eq!(status, 1, "{stdout}");
    for part in [
        "test app::unit::app::fibonacci ... ok\n",
        "test app::unit::app::once ... ok\n",
        "test app::unit::app::plain ... ok\n",
        "--- app::unit::app::fails\nerror: empty\n --> src/main_test.to:33:5\n",
        "panic[P2009]: test-bench-conflict: this test already has a benchmark named \"same\"",
    ] {
        assert!(stdout.contains(part), "missing {part:?} in {stdout}");
    }
    assert_eq!(stdout.matches("ran\n").count(), 1, "{stdout}");
    // The list says which tests are benchmarks.
    let (stdout, _, _) = tondo(&dir, &["test", "--list", "--test-format", "json"]);
    let list: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let marked: Vec<(&str, bool)> = list["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["bench"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        marked,
        [("fails", true), ("fibonacci", true), ("joins", true), ("once", true), ("plain", false), ("twice", true)]
    );
    // `--bench` measures the benchmarks alone.
    let exact = ["--exact", "app::unit::app::fibonacci", "--exact", "app::unit::app::joins", "--exact"];
    let mut args = vec!["test", "--bench", "--bench-time", "0.2"];
    args.extend(exact);
    args.push("app::unit::app::fails");
    let (stdout, _, status) = tondo(&dir, &args);
    assert_eq!(status, 1, "{stdout}");
    assert!(stdout.contains("test app::unit::app::fibonacci ... ok\n  fib 5 "), "{stdout}");
    assert!(stdout.contains("/op  ±") && stdout.contains(" allocs/op  "), "{stdout}");
    args.extend(["--test-format", "json"]);
    let (stdout, _, _) = tondo(&dir, &args);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["bench"]["time_ms"], 200, "{report}");
    assert!(report["bench"]["cpus"].as_u64().unwrap() >= 1);
    let measured = |name: &str| {
        let test = report["tests"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        test["attempts"][0]["benchmarks"].clone()
    };
    let fib = measured("fibonacci");
    let names: Vec<&str> = fib.as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["fib 5", "fib 15"]);
    let median = |b: &serde_json::Value| b["ns_per_op"]["median"].as_f64().unwrap();
    assert!(median(&fib[1]) > median(&fib[0]) * 10.0, "{fib}");
    for benchmark in fib.as_array().unwrap() {
        let samples = benchmark["samples"].as_u64().unwrap();
        assert!((5..=50).contains(&samples), "{benchmark}");
        assert_eq!(benchmark["samples_ns_per_op"].as_array().unwrap().len() as u64, samples);
        assert_eq!(benchmark["allocations_per_op"].as_f64(), Some(0.0), "{benchmark}");
    }
    assert!(measured("joins")[0]["allocations_per_op"].as_f64().unwrap() >= 1.0);
    assert_eq!(measured("fails"), serde_json::json!([]), "a body that fails measures nothing");
    // Benchmarks run alone, optimized and once.
    let (_, stderr, status) = tondo(&dir, &["test", "--bench", "--jobs", "2"]);
    assert_eq!(status, 2, "{stderr}");
    assert!(stderr.contains("--bench measures each benchmark once, alone and optimized"), "{stderr}");
    let (_, stderr, _) = tondo(&dir, &["test", "--bench-time", "1"]);
    assert!(stderr.contains("it goes with --bench"), "{stderr}");
    // Only a test's own body measures.
    std::fs::write(
        dir.join("src/main_test.to"),
        "import std.testing\n\nfn helper() {\n    testing.bench(\"x\", () { 1 })\n}\n\ntest uses {\n    helper()\n}\n",
    )
    .unwrap();
    let (_, stderr, _) = tondo(&dir, &["test"]);
    assert!(stderr.contains("error[E2006]"), "{stderr}");
}

#[test]
fn compares_benchmarks_with_baselines() {
    let tests = "\
import std.testing

test fibonacci {
    testing.bench(\"fib 15\", () { fib(15) })
}
";
    let dir = project("benchmark-baselines", tests);
    let source = |extra: &str| {
        format!(
            "pub fn fib(n: Int): Int {{\n    if n < 2 {{\n        return n\n    }}\n    fib(n - 1) + fib(n - 2){extra}\n}}\n\nfn main() {{}}\n"
        )
    };
    std::fs::write(dir.join("src/main.to"), source("")).unwrap();
    let bench = ["test", "--bench", "--bench-time", "0.2"];
    let (_, stderr, status) = tondo(&dir, &[&bench[..], &["--save-baseline", "main"]].concat());
    assert_eq!(status, 0, "{stderr}");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("target/bench/main.json")).unwrap()).unwrap();
    assert_eq!(saved["format"], "tondo-bench-baseline-0.1/1");
    assert_eq!(saved["benchmarks"][0]["test"], "app::unit::app::fibonacci");
    assert_eq!(saved["benchmarks"][0]["name"], "fib 15");
    // Three times the work is slower, beyond what is allowed.
    std::fs::write(dir.join("src/main.to"), source(" + 0 * (fib(n - 1) + fib(n - 2))")).unwrap();
    let compare = [&bench[..], &["--baseline", "main", "--max-regression", "5"]].concat();
    let (stdout, _, status) = tondo(&dir, &compare);
    assert_eq!(status, 1, "{stdout}");
    assert!(stdout.contains("% slower, REGRESSED\n"), "{stdout}");
    assert!(stdout.contains("benchmarks that regressed beyond 5%:\n  app::unit::app::fibonacci fib 15: "), "{stdout}");
    let (stdout, _, _) = tondo(&dir, &[&compare[..], &["--test-format", "json"]].concat());
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["bench"]["baseline"]["kind"], "saved");
    assert_eq!(report["bench"]["max_regression_percent"].as_f64(), Some(5.0));
    let compared = &report["tests"][0]["attempts"][0]["benchmarks"][0]["baseline"];
    assert_eq!(compared["verdict"], "slower", "{compared}");
    assert!(compared["change_percent"].as_f64().unwrap() > 50.0, "{compared}");
    assert!(compared["regressed"].as_str().unwrap().ends_with("% slower"), "{compared}");
    // Against a revision: the committed one, built and run alongside.
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(&dir).status().unwrap();
        assert!(status.success(), "git {args:?}");
    };
    std::fs::remove_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.join("src/main.to"), source("")).unwrap();
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "fast"]);
    std::fs::write(dir.join("src/main.to"), source(" + 0 * (fib(n - 1) + fib(n - 2))")).unwrap();
    let against = [&bench[..], &["--against", "HEAD", "--max-regression", "5"]].concat();
    let (stdout, stderr, status) = tondo(&dir, &against);
    assert_eq!(status, 1, "{stdout}{stderr}");
    assert!(stderr.contains("tondo: building the tests of HEAD ("), "{stderr}");
    assert!(stdout.contains("% slower, REGRESSED\n"), "{stdout}");
    // The same code is not slower, even on a machine shared with others.
    std::fs::write(dir.join("src/main.to"), source("")).unwrap();
    let (stdout, _, status) = tondo(&dir, &[&bench[..], &["--against", "HEAD", "--max-regression", "100"]].concat());
    assert_eq!(status, 0, "{stdout}");
    // What goes with what.
    for (args, said) in [
        (vec!["test", "--baseline", "main"], "--baseline is about benchmarks: it goes with --bench"),
        (vec!["test", "--bench", "--max-regression", "5"], "--max-regression compares with a baseline"),
        (vec!["test", "--bench", "--baseline", "main", "--against", "HEAD"], "choose one"),
        (vec!["test", "--bench", "--against", "nowhere"], "git knows no revision nowhere"),
    ] {
        let (_, stderr, status) = tondo(&dir, &args);
        assert_ne!(status, 0);
        assert!(stderr.contains(said), "{args:?}: {stderr}");
    }
}

#[test]
fn writes_reports_for_people() {
    let tests = "\
import std.testing

test adds {
    assert(add(1, 2) == 3)
}

test breaks {
    assert(add(1, 1) == 3)
}

test waits {
    testing.skip(\"needs a database\")
}

test adding {
    testing.bench(\"add | sum\", () { add(2, 3) })
}
";
    let dir = project("reports", tests);
    std::fs::write(dir.join("src/main.to"), "pub fn add(a: Int, b: Int): Int {\n    a + b\n}\n\nfn main() {}\n")
        .unwrap();
    let (markdown, html) = (dir.join("report.md"), dir.join("report.html"));
    let reports = [format!("markdown={}", markdown.display()), format!("html={}", html.display())];
    let (_, stderr, status) = tondo(&dir, &["test", "--coverage", "--report", &reports[0], "--report", &reports[1]]);
    assert_eq!(status, 1, "{stderr}");
    let text = std::fs::read_to_string(&markdown).unwrap();
    for part in [
        "# Tests of app\n\n**FAILED: 2 passed, 1 failed, 1 skipped, 0 blocked, in ",
        "## Failures\n\n### `app::unit::app::breaks`\n\n```text\npanic[P0007]: assertion-failed",
        "## Skipped\n\n- `app::unit::app::waits`: needs a database\n",
        "## Coverage\n\n| File | Lines | Functions | Branches |\n|---|---:|---:|---:|\n| `src/main.to` | ",
        "| **Total** | ",
    ] {
        assert!(text.contains(part), "missing {part:?} in {text}");
    }
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.starts_with("<!doctype html>") && !page.contains("<script"), "{page}");
    assert!(page.contains("<h2>Coverage</h2>") && page.contains("assertion-failed"), "{page}");
    // Benchmarks, with their table.
    let (_, stderr, _) =
        tondo(&dir, &["test", "--bench", "--bench-time", "0.1", "--report", &reports[0], "--report", &reports[1]]);
    let text = std::fs::read_to_string(&markdown).unwrap();
    assert!(text.contains("## Benchmarks\n\nMeasured for 0.1 s each on "), "{text}{stderr}");
    assert!(text.contains("| Test | Benchmark | Time/op | ± | Allocs/op | B/op |\n"), "{text}");
    assert!(text.contains("| `adding` | add \\| sum | "), "{text}");
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.contains("<td>add | sum</td>") && page.contains("<svg"), "{page}");
}
