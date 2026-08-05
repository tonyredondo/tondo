use std::io::Write;
use std::process::{Command, Stdio};

use tondo_conformance::protocol::{
    AdapterAction, AdapterRequest, AdapterResponse, AdapterResult, TargetSelection,
};
use tondo_conformance::runner::{Adapter, ProcessAdapter};

fn adapter_executable() -> &'static str {
    env!("CARGO_BIN_EXE_tondo-reference-adapter")
}

fn describe_request() -> AdapterRequest {
    AdapterRequest::new(
        7,
        "process/describe",
        TargetSelection {
            name: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
        },
        AdapterAction::Describe,
    )
}

#[test]
fn process_adapter_round_trips_the_protocol_and_reaps_its_child() {
    let mut adapter =
        ProcessAdapter::spawn(adapter_executable()).expect("the adapter process must start");
    let response = adapter
        .exchange(&describe_request())
        .expect("the process protocol must round-trip");

    assert_eq!(response.sequence, 7);
    assert_eq!(response.case_id, "process/describe");
    assert!(matches!(response.result, AdapterResult::Ok { .. }));
}

#[test]
fn process_adapter_reports_an_unstartable_executable() {
    let missing =
        std::env::temp_dir().join(format!("tondo-missing-adapter-{}", std::process::id()));
    let error = ProcessAdapter::spawn(&missing)
        .err()
        .expect("a missing adapter executable must fail");

    assert!(error.contains("cannot start adapter"));
    assert!(error.contains(&missing.display().to_string()));
}

#[test]
fn stdio_server_rejects_malformed_json_without_losing_framing() {
    let mut child = Command::new(adapter_executable())
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the stdio server must start");
    child
        .stdin
        .take()
        .expect("stdin must be piped")
        .write_all(b"{not-json}\n")
        .expect("the malformed frame must be writable");
    let output = child
        .wait_with_output()
        .expect("the stdio server must terminate at EOF");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: AdapterResponse =
        serde_json::from_slice(&output.stdout).expect("the error response must remain valid JSON");
    assert_eq!(response.sequence, 0);
    assert!(response.case_id.is_empty());
    let AdapterResult::Error { message } = response.result else {
        panic!("malformed JSON must use the protocol error channel");
    };
    assert!(message.contains("invalid request JSON"));
}

#[test]
fn stdio_server_requires_its_explicit_mode() {
    let output = Command::new(adapter_executable())
        .output()
        .expect("the adapter executable must run");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("usage must be UTF-8"),
        "usage: tondo-reference-adapter --stdio\n"
    );
}
