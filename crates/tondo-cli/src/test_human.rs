//! Human projection of the validated report, without rerunning or reclassifying tests.

use std::io::{self, Write};

use serde::Serialize;
use tondo_compiler::test_output::CapturedOutput;
use tondo_compiler::test_report::{ReportOrder, TestList, TestReport};
use tondo_compiler::test_result::{AggregateStatus, SnapshotStatus, TestAttempt, TestNode};

pub fn write_report(
    report: &TestReport,
    show_output: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<()> {
    write_order(stdout, &report.metadata().order)?;
    for node in report.suites().iter().chain(report.tests()) {
        write_node(node, show_output, stdout, stderr)?;
    }
    Ok(())
}

pub fn write_list(list: &TestList, stdout: &mut impl Write) -> io::Result<()> {
    write_order(stdout, &list.metadata().order)?;
    for node in list.suites().iter().chain(list.tests()) {
        writeln!(stdout, "{}", node.id)?;
        write_owners(stdout, &node.owners)?;
    }
    stdout.flush()
}

fn write_order(stdout: &mut impl Write, order: &ReportOrder) -> io::Result<()> {
    if let Some(seed) = &order.seed {
        writeln!(stdout, "order: random, seed: {seed}")?;
    }
    Ok(())
}

fn write_node(
    node: &TestNode,
    show_output: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<()> {
    writeln!(stdout, "{} {}", super::status_label(node.status), node.id)?;
    let blocked = matches!(
        node.status,
        AggregateStatus::BlockedSetup | AggregateStatus::BlockedSkip
    );
    let details = show_output || node.status != AggregateStatus::Passed;
    if details && !blocked {
        write_owners(stdout, &node.owners)?;
    }
    for attempt in &node.attempts {
        if !details && attempt.virtual_time.is_empty() {
            continue;
        }
        write!(
            stdout,
            "  attempt {} (iteration {}, round {}): ",
            attempt.index, attempt.iteration, attempt.round
        )?;
        json_line(stdout, "", &attempt.status)?;
        if let Some(phase) = attempt.phase {
            json_line(stdout, "    phase: ", &phase)?;
        }
        if let Some(cause) = &attempt.blocked_by {
            writeln!(
                stdout,
                "    blocked by {} attempt {}",
                cause.id, cause.attempt
            )?;
            continue;
        }
        if details {
            write_attempt_details(node, attempt, show_output, stdout, stderr)?;
        }
        if !attempt.virtual_time.is_empty() {
            writeln!(
                stdout,
                "    virtual time: {} domains",
                attempt.virtual_time.len()
            )?;
            for domain in &attempt.virtual_time {
                writeln!(
                    stdout,
                    "      domain {}: {} ns final ({} automatic advances, {} explicit advances, {} settles)",
                    domain.index,
                    domain.elapsed_ns,
                    domain.automatic_advances,
                    domain.explicit_advances,
                    domain.settles
                )?;
            }
        }
    }
    stdout.flush()?;
    stderr.flush()
}

fn write_attempt_details(
    node: &TestNode,
    attempt: &TestAttempt,
    show_output: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<()> {
    if let Some(failure) = &attempt.failure {
        json_line(stdout, "    failure: ", failure)?;
    }
    if let Some(skip) = &attempt.skip {
        json_line(stdout, "    skip reason: ", &skip.reason)?;
    }
    if !attempt.tags.is_empty() {
        json_line(stdout, "    tags: ", &attempt.tags)?;
    }
    for artifact in &attempt.artifacts {
        json_line(stdout, "    artifact: ", artifact)?;
    }
    for snapshot in &attempt.snapshots {
        if show_output || snapshot.status != SnapshotStatus::Matched {
            json_line(stdout, "    snapshot: ", snapshot)?;
        }
    }
    for (index, log) in attempt.logs.iter().enumerate() {
        write!(stdout, "    log {}: ", index + 1)?;
        json_line(stdout, "", log)?;
    }
    write_stream(stdout, "stdout", &node.id, attempt.index, &attempt.stdout)?;
    // Flush the first destination before the second: redirected streams may
    // share a terminal, but output from different nodes must never interleave.
    stdout.flush()?;
    write_stream(stderr, "stderr", &node.id, attempt.index, &attempt.stderr)
}

fn write_owners(stdout: &mut impl Write, owners: &[String]) -> io::Result<()> {
    if !owners.is_empty() {
        json_line(stdout, "  owners: ", owners)?;
    }
    Ok(())
}

fn json_line<T: Serialize + ?Sized>(
    stdout: &mut impl Write,
    prefix: &str,
    value: &T,
) -> io::Result<()> {
    stdout.write_all(prefix.as_bytes())?;
    serde_json::to_writer(&mut *stdout, value).map_err(io::Error::other)?;
    writeln!(stdout)
}

fn write_stream(
    output: &mut impl Write,
    name: &str,
    id: &str,
    attempt: u32,
    stream: &CapturedOutput,
) -> io::Result<()> {
    if !stream.is_empty() {
        writeln!(output, "    {name} {id} attempt {attempt}:")?;
        write!(output, "{stream}")?;
        if !stream.as_bytes().ends_with(b"\n") || stream.as_text().is_none() {
            writeln!(output)?;
        }
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tondo_compiler::test_result::{
        ArtifactRecord, AttemptStatus, BlockedBy, ResultNodeKind, SnapshotRecord, VirtualTimeRecord,
    };

    fn node(status: AggregateStatus, attempts: Vec<TestAttempt>) -> TestNode {
        let mut node = TestNode::new(
            "p::integration::tests::leaf",
            None,
            "p",
            ResultNodeKind::Test,
            "tests",
            "leaf",
            attempts,
        );
        node.status = status;
        node.owners = vec!["@owner".into()];
        node
    }

    #[test]
    fn human_attempts_preserve_failure_flaky_and_repeat_evidence() {
        let mut first = TestAttempt::new(1, 1, 0, None, AttemptStatus::FailedPanic);
        first.logs = vec!["first\nlog".into()];
        first.stdout = "first-output".into();
        first.stderr = vec![255].into();
        first.tags.insert("attempt".into(), "first".into());
        first.artifacts.push(ArtifactRecord {
            name: "trace".into(),
            media_type: "text/plain".into(),
            size: 3,
            sha256: "a".repeat(64),
            object: "sha256/trace".into(),
        });
        first.snapshots.push(SnapshotRecord {
            name: "difference".into(),
            status: SnapshotStatus::Mismatched,
            expected_sha256: Some("b".repeat(64)),
            actual_sha256: "c".repeat(64),
        });
        let mut second = TestAttempt::new(2, 2, 0, None, AttemptStatus::Passed);
        second.logs = vec!["second-log".into()];
        second.stdout = "second-output\n".into();
        for status in [AggregateStatus::FailedPanic, AggregateStatus::FlakyPass] {
            second.status = if status == AggregateStatus::FailedPanic {
                AttemptStatus::FailedPanic
            } else {
                AttemptStatus::Passed
            };
            let node = node(status, vec![first.clone(), second.clone()]);
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            write_node(&node, false, &mut stdout, &mut stderr).unwrap();
            let stdout = String::from_utf8(stdout).unwrap();
            assert!(stdout.contains("  owners: [\"@owner\"]"));
            assert!(stdout.contains("attempt 1 (iteration 1, round 0)"));
            assert!(stdout.contains("attempt 2 (iteration 2, round 0)"));
            assert!(stdout.contains("log 1: \"first\\nlog\""));
            assert!(stdout.contains("log 1: \"second-log\""));
            assert!(stdout.contains("tags: {\"attempt\":\"first\"}"));
            assert!(stdout.contains("artifact: {\"name\":\"trace\""));
            assert!(stdout.contains("\"status\":\"mismatched\""));
            assert!(
                stdout.find("first-output\n").unwrap() < stdout.find("second-output\n").unwrap()
            );
            assert_eq!(
                String::from_utf8(stderr).unwrap(),
                "    stderr p::integration::tests::leaf attempt 1:\n[base64] /w==\n"
            );
        }
    }

    #[test]
    fn human_passing_output_is_opt_in_and_blocking_references_its_cause() {
        let mut attempt = TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed);
        attempt.logs = vec!["hidden-log".into()];
        attempt.stdout = "hidden-output".into();
        attempt.tags.insert("hidden-tag".into(), "value".into());
        attempt.snapshots.push(SnapshotRecord {
            name: "matched".into(),
            status: SnapshotStatus::Matched,
            expected_sha256: Some("a".repeat(64)),
            actual_sha256: "a".repeat(64),
        });
        attempt.virtual_time.push(VirtualTimeRecord {
            index: 1,
            elapsed_ns: "42".into(),
            automatic_advances: 2,
            explicit_advances: 3,
            settles: 4,
        });
        let passing = node(AggregateStatus::Passed, vec![attempt]);
        for show_output in [false, true] {
            let mut stdout = Vec::new();
            write_node(&passing, show_output, &mut stdout, &mut Vec::new()).unwrap();
            let stdout = String::from_utf8(stdout).unwrap();
            for hidden in [
                "@owner",
                "hidden-log",
                "hidden-output",
                "hidden-tag",
                "\"matched\"",
            ] {
                assert_eq!(stdout.contains(hidden), show_output, "{stdout}");
            }
            assert!(stdout.contains("virtual time: 1 domains"));
            assert!(stdout.contains(
                "domain 1: 42 ns final (2 automatic advances, 3 explicit advances, 4 settles)"
            ));
        }
        let mut blocked = TestAttempt::new(1, 1, 0, None, AttemptStatus::BlockedSkip);
        blocked.blocked_by = Some(BlockedBy {
            id: "p::integration::tests::suite".into(),
            attempt: 2,
        });
        let blocked = node(AggregateStatus::BlockedSkip, vec![blocked]);
        let mut stdout = Vec::new();
        write_node(&blocked, true, &mut stdout, &mut Vec::new()).unwrap();
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.contains("blocked by p::integration::tests::suite attempt 2"));
        assert!(!stdout.contains("@owner"));
    }

    #[test]
    fn human_report_propagates_destination_failures() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("destination closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut attempt = TestAttempt::new(1, 1, 0, None, AttemptStatus::FailedPanic);
        attempt.stderr = "error".into();
        let node = node(AggregateStatus::FailedPanic, vec![attempt]);
        assert!(write_node(&node, false, &mut Broken, &mut Vec::new()).is_err());
        assert!(write_node(&node, false, &mut io::sink(), &mut Broken).is_err());
    }
}
