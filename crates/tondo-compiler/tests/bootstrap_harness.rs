mod support;

use std::collections::BTreeSet;

use tondo_compiler::driver::{Operation, ResourceLimits, discover_tests, execute};
use tondo_compiler::test_control::{EnvelopeHandle, EnvelopeLimits};
use tondo_compiler::types::{IntrinsicType, TypeInterner};

use support::{FixtureKind, discover, inline_module_request, inline_request, workspace_test_root};

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
        b"fn invalid(): Int { \"text\" - 1 }\n",
    );
    let output = execute(request).unwrap();
    let json = output.diagnostics().json_lines().unwrap();

    assert!(json.contains("\"code\":\"E1102\""));
    assert!(json.contains("\"source_id\":\"root:inline-test\""));
    assert!(json.contains("\"file\":\"inline.to\""));
}

#[test]
fn public_driver_executes_a_fallible_virtual_time_callback() {
    let base = inline_module_request(
        Operation::Check,
        "virtual-time.to",
        b"import std.testing\nimport std.time\ntest virtualClock {\n match await testing.withVirtualTime(async (clock) {\n  scope {\n   let sleeper = spawn time.sleep(time.Duration.fromNanoseconds(3))\n   await clock.settle()\n   _ = await sleeper?\n  }\n }) {\n  ok(_) => ()\n  err(_) => testing.failNow(\"virtual time failed\")\n }\n}\n",
    );
    let entries = discover_tests(&base).unwrap();
    let request = base
        .for_test_entry(&entries[0])
        .unwrap()
        .with_test_envelope(EnvelopeHandle::new(
            "public-virtual-time",
            EnvelopeLimits::new(4096, 4096, 4096),
        ));
    let output = execute(request).unwrap();

    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    assert!(output.diagnostics().is_empty());
}

#[test]
fn virtual_time_has_one_canonical_public_type_name() {
    assert_eq!(IntrinsicType::VirtualTime.to_string(), "VirtualTime");
    assert!(!TypeInterner::default().is_empty());
}

#[test]
fn value_copy_observables_are_stable_under_gc_pressure() {
    let fixtures = discover(FixtureKind::Runtime)
        .unwrap()
        .into_iter()
        .filter(|fixture| {
            fixture
                .source
                .parent()
                .and_then(|parent| parent.file_name())
                .is_some_and(|name| name == "value-copy")
        })
        .collect::<Vec<_>>();
    let names = fixtures
        .iter()
        .filter_map(|fixture| fixture.source.file_stem())
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from([
            "gc-pressure".to_owned(),
            "identity".to_owned(),
            "iteration".to_owned(),
            "map-remove".to_owned(),
            "panic".to_owned(),
            "slice-snapshot".to_owned(),
            "value".to_owned(),
            "write-independence".to_owned(),
        ])
    );

    let pressure_limits = ResourceLimits {
        initial_vm_gc_threshold: 1,
        ..ResourceLimits::default()
    };
    for fixture in fixtures {
        let baseline = fixture.run().unwrap();
        fixture.assert_matches(&baseline).unwrap();

        let under_pressure = fixture.run_with_limits(pressure_limits).unwrap();
        fixture.assert_matches(&under_pressure).unwrap();
        assert_eq!(
            under_pressure,
            baseline,
            "{} changed an observable under GC pressure",
            fixture.source.display()
        );
    }
}

#[test]
fn text_interpolation_observables_are_stable_under_gc_pressure() {
    let fixture = discover(FixtureKind::Runtime)
        .unwrap()
        .into_iter()
        .find(|fixture| {
            fixture
                .source
                .file_stem()
                .is_some_and(|name| name == "m6-text-003-display")
        })
        .expect("TEXT-003 runtime fixture must be discoverable");
    let baseline = fixture.run().unwrap();
    fixture.assert_matches(&baseline).unwrap();

    let under_pressure = fixture
        .run_with_limits(ResourceLimits {
            initial_vm_gc_threshold: 1,
            ..ResourceLimits::default()
        })
        .unwrap();
    fixture.assert_matches(&under_pressure).unwrap();
    assert_eq!(under_pressure, baseline);
}

#[test]
fn variadic_pack_and_spread_observables_are_stable_under_gc_pressure() {
    let fixtures = discover(FixtureKind::Runtime)
        .unwrap()
        .into_iter()
        .filter(|fixture| {
            fixture.source.file_stem().is_some_and(|name| {
                matches!(name.to_str(), Some("m6-variadic-001" | "m6-variadic-002"))
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(fixtures.len(), 2);
    for fixture in fixtures {
        let baseline = fixture.run().unwrap();
        fixture.assert_matches(&baseline).unwrap();

        let under_pressure = fixture
            .run_with_limits(ResourceLimits {
                initial_vm_gc_threshold: 1,
                ..ResourceLimits::default()
            })
            .unwrap();
        fixture.assert_matches(&under_pressure).unwrap();
        assert_eq!(under_pressure, baseline);
    }
}

#[test]
fn suspended_async_frames_and_completed_children_are_stable_under_gc_pressure() {
    let fixtures = discover(FixtureKind::Runtime)
        .unwrap()
        .into_iter()
        .filter(|fixture| {
            fixture.source.file_stem().is_some_and(|name| {
                matches!(
                    name.to_str(),
                    Some("m7-async-gc-roots" | "m7-structured-ref")
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(fixtures.len(), 2);

    for fixture in fixtures {
        let baseline = fixture.run().unwrap();
        fixture.assert_matches(&baseline).unwrap();

        let under_pressure = fixture
            .run_with_limits(ResourceLimits {
                initial_vm_gc_threshold: 1,
                ..ResourceLimits::default()
            })
            .unwrap();
        fixture.assert_matches(&under_pressure).unwrap();
        assert_eq!(
            under_pressure,
            baseline,
            "{} changed an observable while async frames were suspended under GC pressure",
            fixture.source.display()
        );
    }
}
