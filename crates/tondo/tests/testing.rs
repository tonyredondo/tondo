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
    let (_, _, status) = tondo(&dir, &["test"]);
    assert_eq!(status, 3);
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
