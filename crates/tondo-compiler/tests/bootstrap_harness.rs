mod support;

use tondo_compiler::driver::{Operation, execute};

use support::{FixtureKind, discover, inline_request, workspace_test_root};

#[test]
fn all_fixture_classes_are_discoverable() {
    for kind in [
        FixtureKind::Spec,
        FixtureKind::CompilePass,
        FixtureKind::CompileFail,
        FixtureKind::Runtime,
    ] {
        let fixtures = discover(kind).unwrap();
        assert!(
            fixtures
                .windows(2)
                .all(|pair| pair[0].source < pair[1].source)
        );
        for fixture in fixtures {
            assert_eq!(fixture.kind, kind);
            assert_eq!(fixture.sidecar("jsonl").extension().unwrap(), "jsonl");
        }
    }
    assert!(workspace_test_root().is_dir());
}

#[test]
fn repository_fixtures_match_their_sidecars() {
    for kind in [
        FixtureKind::Spec,
        FixtureKind::CompilePass,
        FixtureKind::CompileFail,
        FixtureKind::Runtime,
    ] {
        for fixture in discover(kind).unwrap() {
            let observation = fixture.run().unwrap();
            fixture.assert_matches(&observation).unwrap();
        }
    }
}

#[test]
fn inline_fixture_observes_structured_driver_output() {
    let request = inline_request(
        Operation::Check,
        "inline.to",
        b"fn invalid(): Int { \"text\" }\n",
    );
    let output = execute(request).unwrap();
    let json = output.diagnostics().json_lines().unwrap();

    assert!(json.contains("\"code\":\"E1102\""));
    assert!(json.contains("\"source_id\":\"root:inline-test\""));
    assert!(json.contains("\"file\":\"inline.to\""));
}
