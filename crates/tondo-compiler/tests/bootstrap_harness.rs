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
fn public_rounding_modes_reject_wrong_arity_and_non_float_values() {
    for operation in ["round", "roundTiesAway"] {
        for statement in [
            format!("_ = math.{operation}()"),
            format!("_ = math.{operation}(2.5, 3.5)"),
            format!("let value: Int = 2\n _ = math.{operation}(value)"),
            format!("_ = math.{operation}(true)"),
        ] {
            let source = format!("import std.math\nfn main() {{\n {statement}\n}}\n");
            let output = execute(inline_module_request(
                Operation::Check,
                "rounding-signatures.to",
                source.as_bytes(),
            ))
            .unwrap();
            assert_eq!(output.exit_code(), 1, "{statement}");
            assert!(
                output.diagnostics().human().contains("E11"),
                "{statement}: {}",
                output.diagnostics().human()
            );
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
fn public_host_import_specializes_nested_generic_record_fields() {
    let source = b"import std.sync as concurrent\n\
        type Envelope[T] = { values: Array[T] }\n\
        fn main() {\n\
            let value: Envelope[String] = Envelope { values: [\"payload\"] }\n\
            let shared: concurrent.Array[Envelope[String]] = concurrent.Array[value]\n\
            match shared.get(0) {\n\
                some(item) => assert(item.values.get(0) == some(\"payload\"))\n\
                none => assert(false)\n\
            }\n\
        }\n";
    let output = execute(inline_module_request(
        Operation::Run,
        "generic-host-record.to",
        source,
    ))
    .expect("the verified generic record must cross the host boundary");
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
}

#[test]
fn public_host_import_retains_types_of_inactive_generic_variant_payloads() {
    let source = b"import std.sync as concurrent\n\
        type Payload = { number: Int }\n\
        enum Choice[T] { Empty, Items(Array[T]) }\n\
        fn main() {\n\
            let value: Choice[Payload] = Choice.Empty\n\
            let shared: concurrent.Array[Choice[Payload]] = concurrent.Array[value]\n\
            match shared.get(0) {\n\
                some(_) => ()\n\
                none => assert(false)\n\
            }\n\
        }\n";
    let output = execute(inline_module_request(
        Operation::Run,
        "generic-host-variant.to",
        source,
    ))
    .expect("all declared payloads need concrete trace descriptors");
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
}

#[test]
fn public_generic_union_payloads_require_nominal_discriminators() {
    for arguments in [
        "String, Int",
        "Int, String",
        "Int, Int",
        "Never, Int",
        "Int | String, Bool",
    ] {
        let source = format!(
            "import std.sync as concurrent\n\
            type Choice[A, B] = {{ value: A | B }}\n\
            fn main() {{\n\
                let value: Choice[{arguments}] = Choice {{ value: 42 }}\n\
                let shared: concurrent.Array[Choice[{arguments}]] = concurrent.Array[value]\n\
                match shared.get(0) {{\n some(_) => ()\n none => assert(false)\n }}\n\
            }}\n"
        );
        let output = execute(inline_module_request(
            Operation::Check,
            "generic-host-union.to",
            source.as_bytes(),
        ))
        .unwrap();
        assert_eq!(output.exit_code(), 1);
        assert!(output.diagnostics().human().contains("E1115"));
    }
}

#[test]
fn public_host_import_normalizes_disjoint_nominal_union_payloads() {
    let mut failures = Vec::new();
    for (first, second, value) in [("String", "Int", "\"payload\""), ("Int", "String", "42")] {
        let source = format!(
            "import std.sync as concurrent\n\
            type Cell[A, B] = {{ value: A }}\n\
            type Choice[A, B] = {{ value: Cell[A, Int] | Cell[B, Bool] }}\n\
            fn main() {{\n\
                let value: Choice[{first}, {second}] = Choice {{ value: Cell[{first}, Int] {{ value: {value} }} }}\n\
                let shared: concurrent.Array[Choice[{first}, {second}]] = concurrent.Array[value]\n\
                match shared.get(0) {{\n some(_) => ()\n none => assert(false)\n }}\n\
            }}\n"
        );
        match execute(inline_module_request(
            Operation::Run,
            "generic-host-nominal-union.to",
            source.as_bytes(),
        )) {
            Ok(output) if output.exit_code() == 0 => {}
            Ok(output) => failures.push(format!(
                "{first},{second}: {}",
                output.diagnostics().human()
            )),
            Err(error) => failures.push(format!("{first},{second}: {error}")),
        }
    }
    assert!(
        failures.is_empty(),
        "disjoint nominal union failures: {failures:?}"
    );
}

#[test]
fn public_driver_executes_a_fallible_virtual_time_callback() {
    let base = inline_module_request(
        Operation::Test,
        "virtual-time.to",
        b"import std.testing\nimport std.time\ntest virtualClock {\n match testing.withVirtualTime((clock) {\n  scope {\n   let sleeper = spawn time.sleep(time.Duration.fromNanoseconds(3))\n   clock.settle()\n   _ = await sleeper?\n  }\n }) {\n  ok(_) => ()\n  err(_) => testing.failNow(\"virtual time failed\")\n }\n}\n",
    );
    let entries = discover_tests(&base).unwrap();
    assert_eq!(entries.len(), 1);
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
fn receiver_selected_suspension_rechecks_forward_function_values_and_sync_contracts() {
    for annotation in ["", "@nosuspend\n"] {
        let source = format!(
            "import std.process\n{annotation}fn first() {{\n let work = later\n work()\n}}\nfn later() {{\n _ = process.command(\"compile-only-program\").run()\n}}\nfn main() {{ first() }}\n"
        );
        let output = execute(inline_module_request(
            Operation::Check,
            "late-effects.to",
            source.as_bytes(),
        ))
        .unwrap();
        if annotation.is_empty() {
            assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
            assert!(output.diagnostics().is_empty());
        } else {
            assert_eq!(output.exit_code(), 1);
            assert!(
                output.diagnostics().human().contains("E1601"),
                "{}",
                output.diagnostics().human()
            );
        }
    }
}

#[test]
fn discarded_await_consumes_the_join_and_rejects_a_second_consumption() {
    for (body, success) in [
        ("_ = await pending", true),
        ("let _ = await pending", true),
        ("_ = await pending\n _ = await pending", false),
        ("_ = pending", false),
    ] {
        let source = format!(
            "fn child(): Int suspends {{ 42 }}\nfn main() {{\n scope {{\n let pending = spawn child()\n {body}\n }}\n}}\n"
        );
        let output = execute(inline_module_request(
            Operation::Run,
            "join-discard.to",
            source.as_bytes(),
        ))
        .unwrap();
        assert_eq!(
            output.exit_code() == 0,
            success,
            "{body}: {}",
            output.diagnostics().human()
        );
        if !success {
            assert!(output.diagnostics().human().contains("E140"));
        }
    }
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
