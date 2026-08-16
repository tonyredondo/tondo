use tondo_conformance::protocol::{
    AdapterAction, AdapterRequest, AdapterResult, CompilationState, TargetSelection, WireOperation,
    WireSource, WireSourceAction, WireSourceForm,
};
use tondo_reference_adapter::ReferenceAdapter;

const DIRECT_SOURCE: &str = r#"import std.async

pub fn compute(): Int {
    let pair = async.oneshot[Int, String]()
    var (waiter, completer) = pair
    _ = completer.complete(42)
    let value = waiter.wait()
    match value {
        ok(result) => result
        err(_) => 0
    }
}

fn main() {
    assert(compute() == 42)
}
"#;

const EXPLICIT_SOURCE: &str = r#"import std.async

pub fn compute(): Int {
    let pair = async.oneshot[Int, String]()
    var (waiter, completer) = pair
    _ = completer.complete(42)
    let value = await waiter.wait()
    match value {
        ok(result) => result
        err(_) => 0
    }
}

fn main() {
    assert(compute() == 42)
}
"#;

fn target() -> TargetSelection {
    TargetSelection {
        name: "tondo-vm-hosted".into(),
        profile: "hosted".into(),
        capabilities: vec![
            "clock".into(),
            "console".into(),
            "environment".into(),
            "filesystem".into(),
            "process".into(),
        ],
    }
}

fn source_request(
    case_id: &str,
    operation: WireOperation,
    form: WireSourceForm,
    source: &str,
    include_interface: bool,
) -> AdapterRequest {
    AdapterRequest::new(
        1,
        case_id,
        target(),
        AdapterAction::Source(WireSourceAction {
            operation,
            form,
            root: "main.to".into(),
            sources: vec![WireSource {
                source_id: "suite:suspension-shared".into(),
                module: "main".into(),
                logical_path: "main.to".into(),
                contents_hex: tondo_conformance::encode_hex(source.as_bytes()),
            }],
            warning_profiles: Vec::new(),
            arguments: Vec::new(),
            gc_threshold: None,
            include_interface,
        }),
    )
}

fn observation(request: AdapterRequest) -> tondo_conformance::protocol::Observation {
    let mut adapter = ReferenceAdapter;
    let response = adapter.handle(&request);
    match response.result {
        AdapterResult::Ok { observation } => observation,
        AdapterResult::Error { message } => panic!("adapter rejected request: {message}"),
        AdapterResult::Unsupported { reason } => {
            panic!("adapter does not support request: {reason}")
        }
    }
}

#[test]
fn direct_and_explicit_wait_have_the_same_suspends_interface_hash() {
    let direct = observation(source_request(
        "suspension-direct",
        WireOperation::Check,
        WireSourceForm::Module,
        DIRECT_SOURCE,
        true,
    ));
    let explicit = observation(source_request(
        "suspension-explicit",
        WireOperation::Check,
        WireSourceForm::Module,
        EXPLICIT_SOURCE,
        true,
    ));

    assert_eq!(direct.compilation, CompilationState::Success);
    assert_eq!(explicit.compilation, CompilationState::Success);
    assert!(direct.diagnostics.is_empty());
    assert!(explicit.diagnostics.is_empty());
    assert_eq!(direct.data["schema"], "tondo-interface-observation-0.1/1");
    assert_eq!(
        direct.data["api_hash"],
        "sha256:43cba38fdd82f712feb7841a716926862275cd2f748a937d18a4f26dc41b6b52"
    );
    assert_eq!(direct.data["api_hash"], explicit.data["api_hash"]);
    assert_eq!(direct.data["content_hash"], explicit.data["content_hash"]);
}

#[test]
fn sync_boundaries_reject_direct_and_explicit_suspension_with_e1601() {
    for (index, attribute) in ["@sync", "@nosuspend"].into_iter().enumerate() {
        let source = format!(
            r#"import std.async

{attribute}
fn invalid() {{
    let pair = async.oneshot[Int, String]()
    var (waiter, completer) = pair
    _ = completer.complete(42)
    let value = waiter.wait()
    _ = value
}}
"#
        );
        let observation = observation(source_request(
            &format!("suspension-sync-{index}"),
            WireOperation::Check,
            WireSourceForm::Module,
            &source,
            false,
        ));
        assert_eq!(observation.compilation, CompilationState::Rejected);
        assert_eq!(
            observation
                .diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic["code"].as_str())
                .collect::<Vec<_>>(),
            ["E1601"]
        );
    }
}

#[test]
fn runtime_direct_suspension_uses_the_same_interface_observation() {
    let observation = observation(source_request(
        "suspension-runtime",
        WireOperation::Run,
        WireSourceForm::Script,
        DIRECT_SOURCE,
        true,
    ));
    assert_eq!(observation.compilation, CompilationState::Success);
    assert_eq!(observation.exit_code, 0);
    assert_eq!(
        observation.data["schema"],
        "tondo-interface-observation-0.1/1"
    );
    assert_eq!(
        observation.data["api_hash"],
        "sha256:43cba38fdd82f712feb7841a716926862275cd2f748a937d18a4f26dc41b6b52"
    );
}
