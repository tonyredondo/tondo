#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_stdlib::uuid::{Uuid, UuidError, UuidLimits};

// Include the independent std-only oracle without the compiler or VM graph.
#[path = "../../crates/tondo-reliability/src/uuid_model.rs"]
mod uuid_model;

use uuid_model::{ReferenceError, ReferenceErrorKind, ReferenceProviders, ReferenceUuid};

type Observation = Result<[u8; 16], (String, Option<usize>)>;

fn reference(value: Result<ReferenceUuid, ReferenceError>) -> Observation {
    value
        .map(ReferenceUuid::to_bytes)
        .map_err(|error| (format!("{:?}", error.kind), error.offset))
}

fn kernel(value: Result<Uuid, UuidError>) -> Observation {
    value
        .map(Uuid::to_bytes)
        .map_err(|error| (format!("{:?}", error.kind), error.offset))
}

fn compare_generated(seed: u64) {
    let case = uuid_model::case_from_seed(seed);
    let expected = ReferenceUuid::from_bytes(&case.bytes).unwrap();
    let actual = Uuid::from_bytes(&case.bytes).unwrap();
    assert_eq!(actual.to_bytes(), expected.to_bytes());
    assert_eq!(actual.try_to_string().unwrap(), expected.canonical());
    assert_eq!(
        kernel(Uuid::parse(&expected.canonical())),
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
        kernel(Uuid::v4(&case.entropy)),
        reference(ReferenceUuid::v4(&case.entropy))
    );
    assert_eq!(
        kernel(Uuid::v5(
            actual,
            &case.name,
            UuidLimits {
                max_name_bytes: case.name_limit
            }
        )),
        reference(expected.v5(&case.name, case.name_limit))
    );
    assert_eq!(
        kernel(Uuid::v7(case.milliseconds, &case.entropy[..10])),
        reference(ReferenceUuid::v7(case.milliseconds, &case.entropy[..10]))
    );

    let mut providers = ReferenceProviders::new(
        vec![Ok(case.milliseconds)],
        vec![Ok(case.entropy.to_vec()), Ok(case.entropy[..10].to_vec())],
    )
    .unwrap();
    assert_eq!(
        reference(providers.v4(15)),
        Err(("ResourceLimit".to_owned(), None))
    );
    assert_eq!(providers.consumed(), (0, 0));
    assert_eq!(reference(providers.v4(16)), kernel(Uuid::v4(&case.entropy)));
    assert_eq!(
        reference(providers.v7(10)),
        kernel(Uuid::v7(case.milliseconds, &case.entropy[..10]))
    );
    assert_eq!(
        providers.consumed(),
        (
            1,
            if (0..=uuid_model::MAX_MILLISECONDS).contains(&case.milliseconds) {
                2
            } else {
                1
            }
        )
    );
    providers.close();
    assert_eq!(
        providers.v4(16).unwrap_err().kind,
        ReferenceErrorKind::ProviderMisconfigured
    );
    let mut failures = ReferenceProviders::new(
        vec![Err(ReferenceErrorKind::ClockFailure)],
        vec![Err(ReferenceErrorKind::EntropyFailure)],
    )
    .unwrap();
    assert_eq!(
        failures.v7(10).unwrap_err().kind,
        ReferenceErrorKind::ClockFailure
    );
    assert_eq!(failures.consumed(), (1, 0));
    assert_eq!(
        failures.v4(16).unwrap_err().kind,
        ReferenceErrorKind::EntropyFailure
    );
    assert_eq!(failures.consumed(), (1, 1));
}

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(uuid_model::MAX_UUID_FUZZ_INPUT_BYTES)];
    let summary = uuid_model::run_uuid_fuzz_case(input).unwrap();
    assert_eq!(summary, uuid_model::run_uuid_fuzz_case(input).unwrap());
    assert!((1..=uuid_model::MAX_UUID_FUZZ_STEPS).contains(&summary.steps));
    assert_eq!(summary.generated + summary.refusals, summary.steps * 3);
    for step in 0..summary.steps {
        compare_generated(uuid_model::seed_at_step(input, step));
    }

    let body = input.get(1..).unwrap_or_default();
    let (actual, expected) = match input.first().copied().unwrap_or(0) % 5 {
        0 => {
            let text = String::from_utf8_lossy(body);
            (
                kernel(Uuid::parse(&text)),
                reference(ReferenceUuid::parse(&text)),
            )
        }
        1 => (
            kernel(Uuid::from_bytes(body)),
            reference(ReferenceUuid::from_bytes(body)),
        ),
        2 => (kernel(Uuid::v4(body)), reference(ReferenceUuid::v4(body))),
        3 => {
            let limit = body.get(..2).map_or(0, |bytes| {
                usize::from(u16::from_be_bytes(bytes.try_into().unwrap()))
            });
            let namespace = body.get(2..18).unwrap_or(&[0; 16]);
            let name = body.get(18..).unwrap_or_default();
            let actual = kernel(Uuid::v5(
                Uuid::from_bytes(namespace).unwrap(),
                name,
                UuidLimits {
                    max_name_bytes: limit,
                },
            ));
            assert_eq!(
                actual,
                kernel(Uuid::v5(
                    Uuid::from_bytes(namespace).unwrap(),
                    name,
                    UuidLimits {
                        max_name_bytes: limit
                    }
                ))
            );
            let expected = ReferenceUuid::from_bytes(namespace)
                .unwrap()
                .v5(name, limit);
            // A finite reference cannot reject a production name outside its domain.
            if expected.is_err_and(|error| error.kind == ReferenceErrorKind::OutsideDomain) {
                if name.len() > limit {
                    assert_eq!(actual, Err(("NameLimitExceeded".to_owned(), None)));
                } else {
                    assert!(actual.is_ok());
                }
                return;
            }
            (actual, reference(expected))
        }
        _ => {
            let milliseconds = body
                .get(..16)
                .map_or(0, |bytes| i128::from_be_bytes(bytes.try_into().unwrap()));
            let entropy = body.get(16..).unwrap_or_default();
            (
                kernel(Uuid::v7(milliseconds, entropy)),
                reference(ReferenceUuid::v7(milliseconds, entropy)),
            )
        }
    };
    assert_eq!(actual, expected);
    if let Ok(bytes) = actual {
        let value = Uuid::from_bytes(&bytes).unwrap();
        assert_eq!(
            kernel(Uuid::parse(&value.try_to_string().unwrap())),
            Ok(bytes)
        );
    }
});
