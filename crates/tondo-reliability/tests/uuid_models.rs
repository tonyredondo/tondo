use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use tondo_reliability::harness::decode_hex;
use tondo_reliability::uuid_model::{
    self as model, ReferenceError, ReferenceErrorKind as Kind, ReferenceProviders, ReferenceUuid,
};
use tondo_stdlib::uuid::{Uuid, UuidError, UuidLimits};

fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/uuid-cases.json")).unwrap()
}

fn bytes(row: &Value, field: &str) -> Vec<u8> {
    decode_hex(row[field].as_str().unwrap()).unwrap()
}

fn reference(row: &Value) -> Result<ReferenceUuid, ReferenceError> {
    match row["operation"].as_str().unwrap() {
        "parse" => ReferenceUuid::parse(row["text"].as_str().unwrap()),
        "bytes" => ReferenceUuid::from_bytes(&bytes(row, "bytes_hex")),
        "v4" => ReferenceUuid::v4(&bytes(row, "entropy_hex")),
        "v5" => ReferenceUuid::parse(row["namespace"].as_str().unwrap())
            .unwrap()
            .v5(
                &bytes(row, "name_hex"),
                row["name_limit"].as_u64().unwrap() as usize,
            ),
        "v7" => ReferenceUuid::v7(
            i128::from(row["milliseconds"].as_i64().unwrap()),
            &bytes(row, "entropy_hex"),
        ),
        operation => panic!("unexpected fixture operation {operation}"),
    }
}

fn kernel(row: &Value) -> Result<Uuid, UuidError> {
    match row["operation"].as_str().unwrap() {
        "parse" => Uuid::parse(row["text"].as_str().unwrap()),
        "bytes" => Uuid::from_bytes(&bytes(row, "bytes_hex")),
        "v4" => Uuid::v4(&bytes(row, "entropy_hex")),
        "v5" => Uuid::v5(
            Uuid::parse(row["namespace"].as_str().unwrap()).unwrap(),
            &bytes(row, "name_hex"),
            UuidLimits {
                max_name_bytes: row["name_limit"].as_u64().unwrap() as usize,
            },
        ),
        "v7" => Uuid::v7(
            i128::from(row["milliseconds"].as_i64().unwrap()),
            &bytes(row, "entropy_hex"),
        ),
        operation => panic!("unexpected fixture operation {operation}"),
    }
}

type Observation = Result<[u8; 16], (String, Option<usize>)>;

fn observed_reference(value: Result<ReferenceUuid, ReferenceError>) -> Observation {
    value
        .map(ReferenceUuid::to_bytes)
        .map_err(|error| (format!("{:?}", error.kind), error.offset))
}

fn observed_kernel(value: Result<Uuid, UuidError>) -> Observation {
    value
        .map(Uuid::to_bytes)
        .map_err(|error| (format!("{:?}", error.kind), error.offset))
}

fn compare_seed(seed: u64) {
    let case = model::case_from_seed(seed);
    let expected = ReferenceUuid::from_bytes(&case.bytes).unwrap();
    let actual = Uuid::from_bytes(&case.bytes).unwrap();
    assert_eq!(actual.to_bytes(), expected.to_bytes(), "seed {seed}");
    assert_eq!(
        actual.try_to_string().unwrap(),
        expected.canonical(),
        "seed {seed}"
    );
    assert_eq!(
        observed_kernel(Uuid::parse(&expected.canonical())),
        Ok(expected.to_bytes())
    );
    assert_eq!(actual.version(), expected.version());
    assert_eq!(format!("{:?}", actual.variant()), expected.variant());
    assert_eq!(actual.is_nil(), expected == ReferenceUuid::nil());
    assert_eq!(actual.is_max(), expected == ReferenceUuid::max());
    assert_eq!(
        actual.compare(Uuid::from_bytes(&case.other).unwrap()),
        expected.compare(ReferenceUuid::from_bytes(&case.other).unwrap())
    );
    assert_eq!(
        observed_kernel(Uuid::v4(&case.entropy)),
        observed_reference(ReferenceUuid::v4(&case.entropy))
    );
    assert_eq!(
        observed_kernel(Uuid::v5(
            actual,
            &case.name,
            UuidLimits {
                max_name_bytes: case.name_limit
            }
        )),
        observed_reference(expected.v5(&case.name, case.name_limit)),
        "v5 seed {seed}"
    );
    assert_eq!(
        observed_kernel(Uuid::v7(case.milliseconds, &case.entropy[..10])),
        observed_reference(ReferenceUuid::v7(case.milliseconds, &case.entropy[..10])),
        "v7 seed {seed}"
    );
}

#[test]
fn retained_valid_vectors_match_the_independent_reference_and_kernel() {
    let fixtures = corpus();
    assert_eq!(fixtures["valid"].as_array().unwrap().len(), 17);
    for row in fixtures["valid"].as_array().unwrap() {
        let expected = ReferenceUuid::parse(row["value"].as_str().unwrap()).unwrap();
        let reference = reference(row).unwrap();
        let kernel = kernel(row).unwrap();
        assert_eq!(reference, expected, "{}", row["id"]);
        assert_eq!(kernel.to_bytes(), expected.to_bytes(), "{}", row["id"]);
        assert_eq!(
            kernel.try_to_string().unwrap(),
            row["value"].as_str().unwrap()
        );
        assert_eq!(reference.canonical(), row["value"].as_str().unwrap());
        assert_eq!(reference.version(), row["version"].as_u64().unwrap() as u8);
        assert_eq!(reference.variant(), row["variant"].as_str().unwrap());
    }
}

#[test]
fn retained_invalid_vectors_preserve_nominal_kinds_and_absolute_offsets() {
    let fixtures = corpus();
    assert_eq!(fixtures["invalid"].as_array().unwrap().len(), 37);
    let ids = fixtures["valid"]
        .as_array()
        .unwrap()
        .iter()
        .chain(fixtures["invalid"].as_array().unwrap())
        .map(|row| row["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 54);
    for row in fixtures["invalid"].as_array().unwrap() {
        let expected = Err((
            row["error"].as_str().unwrap().to_owned(),
            row["offset"].as_u64().map(|offset| offset as usize),
        ));
        assert_eq!(
            observed_reference(reference(row)),
            expected,
            "{}",
            row["id"]
        );
        assert_eq!(observed_kernel(kernel(row)), expected, "{}", row["id"]);
    }
}

#[test]
fn generated_uuid_operations_compare_4096_deterministic_seeds() {
    for seed in 0..4096 {
        assert_eq!(model::case_from_seed(seed), model::case_from_seed(seed));
        compare_seed(seed);
    }
}

#[test]
fn every_external_version_variant_and_unsigned_byte_position_is_preserved() {
    for version in 0..16 {
        for variant in 0..=255 {
            let mut bytes = [255; 16];
            bytes[6] = (version << 4) | 15;
            bytes[8] = variant;
            let reference = ReferenceUuid::from_bytes(&bytes).unwrap();
            let kernel = Uuid::from_bytes(&bytes).unwrap();
            assert_eq!(kernel.version(), reference.version());
            assert_eq!(format!("{:?}", kernel.variant()), reference.variant());
            assert_eq!(
                observed_kernel(Uuid::parse(&reference.canonical())),
                Ok(bytes)
            );
        }
    }
    for position in 0..16 {
        for byte in 0..=255 {
            let mut bytes = [0; 16];
            bytes[position] = byte;
            let reference = ReferenceUuid::from_bytes(&bytes).unwrap();
            let kernel = Uuid::from_bytes(&bytes).unwrap();
            assert_eq!(
                kernel.compare(Uuid::nil()),
                reference.compare(ReferenceUuid::nil())
            );
            assert_eq!(
                kernel.compare(Uuid::max()),
                reference.compare(ReferenceUuid::max())
            );
            assert_eq!(kernel.to_bytes(), bytes);
        }
    }
}

#[test]
fn lexical_mutations_compare_every_ascii_position_and_utf8_boundary() {
    let canonical = model::case_from_seed(7).bytes;
    let canonical = ReferenceUuid::from_bytes(&canonical).unwrap().canonical();
    for source in [canonical.clone(), format!("UrN:UuId:{canonical}")] {
        for index in 0..source.len() {
            for byte in 0..=127 {
                let mut changed = source.as_bytes().to_vec();
                changed[index] = byte;
                let text = String::from_utf8(changed).unwrap();
                assert_eq!(
                    observed_kernel(Uuid::parse(&text)),
                    observed_reference(ReferenceUuid::parse(&text)),
                    "{index}/{byte}"
                );
            }
        }
    }
    for index in 0..35 {
        let text = format!("{}é{}", &canonical[..index], &canonical[index + 2..]);
        assert_eq!(
            observed_kernel(Uuid::parse(&text)),
            observed_reference(ReferenceUuid::parse(&text)),
            "UTF-8 {index}"
        );
    }
}

#[test]
fn ignored_layout_bits_collide_and_every_retained_entropy_bit_changes_the_value() {
    let original = model::case_from_seed(11).entropy;
    let base = Uuid::v4(&original).unwrap();
    for ignored in 0..64 {
        let mut bytes = original;
        bytes[6] = (bytes[6] & 15) | ((ignored & 15) << 4);
        bytes[8] = (bytes[8] & 63) | ((ignored >> 4) << 6);
        assert_eq!(Uuid::v4(&bytes).unwrap(), base);
        assert_eq!(
            ReferenceUuid::v4(&bytes).unwrap().to_bytes(),
            base.to_bytes()
        );
    }
    let mut distinct = BTreeSet::from([base]);
    for bit in 0..128 {
        if (bit / 8 == 6 && bit % 8 >= 4) || (bit / 8 == 8 && bit % 8 >= 6) {
            continue;
        }
        let mut bytes = original;
        bytes[bit / 8] ^= 1 << (bit % 8);
        let value = Uuid::v4(&bytes).unwrap();
        assert_ne!(value, base);
        assert_eq!(
            value.to_bytes(),
            ReferenceUuid::v4(&bytes).unwrap().to_bytes()
        );
        distinct.insert(value);
    }
    assert_eq!(distinct.len(), 123);
    let base = Uuid::v7(1, &original[..10]).unwrap();
    for ignored in 0..64 {
        let mut bytes = original[..10].to_vec();
        bytes[0] = (bytes[0] & 15) | ((ignored & 15) << 4);
        bytes[2] = (bytes[2] & 63) | ((ignored >> 4) << 6);
        assert_eq!(Uuid::v7(1, &bytes).unwrap(), base);
        assert_eq!(
            ReferenceUuid::v7(1, &bytes).unwrap().to_bytes(),
            base.to_bytes()
        );
    }
    let mut distinct = BTreeSet::from([base]);
    for bit in 0..80 {
        if (bit / 8 == 0 && bit % 8 >= 4) || (bit / 8 == 2 && bit % 8 >= 6) {
            continue;
        }
        let mut bytes = original[..10].to_vec();
        bytes[bit / 8] ^= 1 << (bit % 8);
        let value = Uuid::v7(1, &bytes).unwrap();
        assert_ne!(value, base);
        assert_eq!(
            value.to_bytes(),
            ReferenceUuid::v7(1, &bytes).unwrap().to_bytes()
        );
        distinct.insert(value);
    }
    assert_eq!(distinct.len(), 75);
    assert_eq!(Uuid::v7(1, &original[..10]), Uuid::v7(1, &original[..10]));
    assert!(Uuid::v7(2, &original[..10]).unwrap() > base);
    assert!(Uuid::v7(0, &original[..10]).unwrap() < base);
}

#[test]
fn independent_v5_digest_covers_padding_boundaries_and_reference_domain_refusal() {
    let namespace = ReferenceUuid::parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let kernel = Uuid::from_bytes(&namespace.to_bytes()).unwrap();
    for length in 0..=96 {
        let name = (0..length)
            .map(|index| (index * 73) as u8)
            .collect::<Vec<_>>();
        assert_eq!(
            observed_reference(namespace.v5(&name, length)),
            observed_kernel(Uuid::v5(
                kernel,
                &name,
                UuidLimits {
                    max_name_bytes: length
                }
            ))
        );
        if length > 0 {
            assert_eq!(
                observed_reference(namespace.v5(&name, length - 1)),
                observed_kernel(Uuid::v5(
                    kernel,
                    &name,
                    UuidLimits {
                        max_name_bytes: length - 1
                    }
                ))
            );
        }
    }
    let outside = vec![0; 97];
    assert_eq!(
        namespace.v5(&outside, 100).unwrap_err().kind,
        Kind::OutsideDomain
    );
    assert!(
        Uuid::v5(
            kernel,
            &outside,
            UuidLimits {
                max_name_bytes: 100
            }
        )
        .is_ok()
    );
    assert_ne!(
        namespace.v5("é".as_bytes(), 96).unwrap(),
        namespace.v5("e\u{301}".as_bytes(), 96).unwrap()
    );
}

#[test]
fn immutable_model_and_kernel_values_keep_copies_keys_and_threaded_observations() {
    let values = Arc::new(
        (0..32)
            .map(|seed| {
                let mut bytes = model::case_from_seed(seed).bytes;
                let reference = ReferenceUuid::from_bytes(&bytes).unwrap();
                let kernel = Uuid::from_bytes(&bytes).unwrap();
                bytes.fill(0);
                let mut output = kernel.to_bytes();
                output.fill(255);
                assert_eq!(kernel.to_bytes(), reference.to_bytes());
                (reference, kernel)
            })
            .collect::<Vec<_>>(),
    );
    for thread in (0..4)
        .map(|_| {
            let values = Arc::clone(&values);
            std::thread::spawn(move || {
                let references = values
                    .iter()
                    .map(|(reference, _)| *reference)
                    .collect::<HashSet<_>>();
                let kernels = values
                    .iter()
                    .map(|(_, kernel)| *kernel)
                    .collect::<HashSet<_>>();
                assert_eq!(references.len(), kernels.len());
                let keys = values
                    .iter()
                    .map(|(reference, kernel)| (*kernel, reference.to_bytes()))
                    .collect::<HashMap<_, _>>();
                for (reference, kernel) in values.iter() {
                    assert_eq!(keys[kernel], reference.to_bytes());
                    assert_eq!(
                        observed_kernel(Uuid::parse(&reference.canonical())),
                        Ok(reference.to_bytes())
                    );
                }
            })
        })
        .collect::<Vec<_>>()
    {
        thread.join().unwrap();
    }
}

#[test]
fn provider_reference_enforces_bounds_consumption_limits_and_terminal_close() {
    assert_eq!(
        ReferenceProviders::new(vec![Ok(0); 17], vec![])
            .unwrap_err()
            .kind,
        Kind::OutsideDomain
    );
    assert_eq!(
        ReferenceProviders::new(vec![], vec![Ok(vec![0; 16]); 17])
            .unwrap_err()
            .kind,
        Kind::OutsideDomain
    );
    for (clocks, entropy) in [
        (vec![Err(Kind::EntropyFailure)], vec![]),
        (vec![], vec![Err(Kind::ClockFailure)]),
        (vec![], vec![Ok(vec![0; 17])]),
    ] {
        assert_eq!(
            ReferenceProviders::new(clocks, entropy).unwrap_err().kind,
            Kind::ProviderMisconfigured
        );
    }
    let mut provider =
        ReferenceProviders::new(vec![Ok(-1), Ok(0)], vec![Ok(vec![0; 16]), Ok(vec![0; 9])])
            .unwrap();
    assert_eq!(provider.v4(15).unwrap_err().kind, Kind::ResourceLimit);
    assert_eq!(provider.v7(9).unwrap_err().kind, Kind::ResourceLimit);
    assert_eq!(provider.consumed(), (0, 0));
    assert_eq!(provider.v7(16).unwrap_err().kind, Kind::TimestampOutOfRange);
    assert_eq!(provider.consumed(), (1, 0));
    provider.v4(16).unwrap();
    assert_eq!(
        provider.v7(16).unwrap_err().kind,
        Kind::ProviderMisconfigured
    );
    assert_eq!(provider.consumed(), (2, 2));
    assert_eq!(provider.v4(16).unwrap_err().kind, Kind::EntropyUnavailable);
    assert_eq!(provider.v7(16).unwrap_err().kind, Kind::ClockUnavailable);
    provider.close();
    assert_eq!(
        provider.v4(16).unwrap_err().kind,
        Kind::ProviderMisconfigured
    );
    assert_eq!(
        provider.v7(16).unwrap_err().kind,
        Kind::ProviderMisconfigured
    );
}

#[test]
fn sealed_provider_transcript_matches_the_independent_model_in_the_production_vm() {
    use std::time::{Duration, UNIX_EPOCH};
    use tondo_compiler::driver::{
        BuildTarget, CompilationRequest, DiagnosticFormat, Edition, HostProfile, Operation,
        ResourceLimits, SourceForm, discover_tests, execute,
    };
    use tondo_compiler::package::PackageGraph;
    use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use tondo_compiler::test_control::{
        EnvelopeHandle, EnvelopeLimits, ExecutionPhase, UuidTestProviders,
    };
    use tondo_stdlib::uuid::UuidErrorKind;

    let r4 = decode_hex("919108f752d133205bacf847db4148a8").unwrap();
    let r7 = decode_hex("0cc318c4dc0c0c07398f").unwrap();
    let clocks = vec![
        Ok(-1),
        Err(Kind::ClockFailure),
        Ok(1_645_557_742_000),
        Ok(1_645_557_742_000),
        Ok(0),
    ];
    let entropy = vec![
        Err(Kind::EntropyFailure),
        Ok(r4.clone()),
        Ok(r7.clone()),
        Ok(r7.clone()),
        Ok(vec![]),
        Ok(vec![0; 10]),
    ];
    let mut reference = ReferenceProviders::new(clocks, entropy).unwrap();
    let operations = [4, 7, 7, 4, 7, 7, 4, 7, 4, 7];
    let mut source = String::from(
        "import std.uuid\nimport std.testing\nfn describe(value: uuid.Uuid ! uuid.UuidError): String {\n match value {\n  ok(item) => item.toString()\n  err(failure) => Display.display(failure)\n }\n}\ntest transcript {\n",
    );
    for operation in operations {
        let result = if operation == 4 {
            reference.v4(16)
        } else {
            reference.v7(16)
        };
        let expected = match result {
            Ok(value) => value.canonical(),
            Err(error) => error.to_string(),
        };
        source.push_str(&format!(
            " testing.assertTextEqual(describe(uuid.Uuid.v{operation}()), \"{expected}\")\n"
        ));
    }
    source.push_str("}\n");
    assert_eq!(reference.consumed(), (5, 6));
    let envelope = EnvelopeHandle::new("uuid-model", EnvelopeLimits::new(65536, 65536, 65536));
    envelope
        .with_uuid_providers(
            UuidTestProviders::new(
                vec![
                    Ok(UNIX_EPOCH - Duration::from_millis(1)),
                    Err(UuidErrorKind::ClockFailure),
                    Ok(UNIX_EPOCH + Duration::from_millis(1_645_557_742_000)),
                    Ok(UNIX_EPOCH + Duration::from_millis(1_645_557_742_000)),
                    Ok(UNIX_EPOCH),
                ],
                vec![
                    Err(UuidErrorKind::EntropyFailure),
                    Ok(r4),
                    Ok(r7.clone()),
                    Ok(r7),
                    Ok(vec![]),
                    Ok(vec![0; 10]),
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:uuid-model").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("uuid-model.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes()),
        ))
        .unwrap();
    let base = CompilationRequest::new(
        Operation::Test,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BuildTarget::vm_hosted_capabilities(),
        DiagnosticFormat::Json,
        SourceForm::Module,
        ResourceLimits::default(),
        PackageGraph::loose(&sources, root).unwrap(),
        sources,
        root,
    )
    .unwrap();
    let entries = discover_tests(&base).unwrap();
    assert_eq!(entries.len(), 1);
    envelope.set_phase(ExecutionPhase::Body).unwrap();
    let output = execute(
        base.for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope),
    )
    .unwrap();
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
}

#[test]
fn uuid_replay_is_deterministic_finite_and_ignores_bytes_outside_its_domain() {
    for input in [
        vec![],
        b"uuid-reference".to_vec(),
        vec![255; 4096],
        vec![23; 5000],
    ] {
        let first = model::run_uuid_fuzz_case(&input).unwrap();
        assert_eq!(first, model::run_uuid_fuzz_case(&input).unwrap());
        assert!(first.steps >= 1 && first.steps <= 512);
        assert_eq!(first.generated + first.refusals, first.steps * 3);
        assert_eq!(
            model::run_uuid_fuzz_case(&input[..input.len().min(4096)]).unwrap(),
            first
        );
        for step in 0..first.steps.min(16) {
            compare_seed(model::seed_at_step(&input, step));
        }
    }
}
