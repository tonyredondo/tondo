//! Explicit OS providers and bounded, sealed UUID test snapshots.

use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};
use tondo_stdlib::uuid::{MAX_UNIX_MILLISECONDS, UuidError, UuidErrorKind};

pub(crate) fn error(kind: UuidErrorKind) -> UuidError {
    UuidError { kind, offset: None }
}

pub(crate) fn unix_milliseconds(now: SystemTime) -> Result<i128, UuidError> {
    let milliseconds = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error(UuidErrorKind::TimestampOutOfRange))?
        .as_millis();
    if milliseconds > MAX_UNIX_MILLISECONDS as u128 {
        return Err(error(UuidErrorKind::TimestampOutOfRange));
    }
    Ok(milliseconds as i128)
}

pub(crate) fn os_entropy(bytes: &mut [u8]) -> Result<(), UuidError> {
    getrandom::fill(bytes).map_err(entropy_error)
}

fn entropy_error(failure: getrandom::Error) -> UuidError {
    error(if failure == getrandom::Error::UNSUPPORTED {
        UuidErrorKind::EntropyUnavailable
    } else {
        UuidErrorKind::EntropyFailure
    })
}

/// Runtime-owned test inputs; Tondo code cannot construct or access this object.
/// Each snapshot is consumed once. Exhaustion refuses rather than using the OS.
#[derive(Debug, Clone)]
pub struct UuidTestProviders {
    clocks: VecDeque<Result<SystemTime, UuidErrorKind>>,
    entropy: VecDeque<Result<Vec<u8>, UuidErrorKind>>,
    logical_bytes: u64,
}

impl UuidTestProviders {
    /// Seal at most 256 snapshots of each provider, with at most sixteen bytes
    /// in each entropy snapshot. Short snapshots exercise nominal refusal.
    pub fn new(
        clocks: Vec<Result<SystemTime, UuidErrorKind>>,
        entropy: Vec<Result<Vec<u8>, UuidErrorKind>>,
    ) -> Result<Self, UuidError> {
        if clocks.len() > 256 || entropy.len() > 256 {
            return Err(error(UuidErrorKind::ResourceLimit));
        }
        if clocks.iter().any(|value| {
            matches!(value, Err(kind)
            if !matches!(kind, UuidErrorKind::ClockUnavailable | UuidErrorKind::ClockFailure))
        }) || entropy.iter().any(|value| match value {
            Ok(bytes) => bytes.len() > 16,
            Err(kind) => !matches!(
                kind,
                UuidErrorKind::EntropyUnavailable | UuidErrorKind::EntropyFailure
            ),
        }) {
            return Err(error(UuidErrorKind::ProviderMisconfigured));
        }
        // Logical fixture storage includes row descriptors and supplied bytes;
        // it is neither allocator instrumentation nor an RSS measurement.
        let logical_bytes = 64
            + 32 * (clocks.len() + entropy.len()) as u64
            + entropy
                .iter()
                .filter_map(|value| value.as_ref().ok())
                .map(|bytes| bytes.len() as u64)
                .sum::<u64>();
        Ok(Self {
            clocks: clocks.into(),
            entropy: entropy.into(),
            logical_bytes,
        })
    }

    pub(crate) fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }

    pub(crate) fn clock(&mut self) -> Result<SystemTime, UuidError> {
        self.clocks
            .pop_front()
            .unwrap_or(Err(UuidErrorKind::ClockUnavailable))
            .map_err(error)
    }

    pub(crate) fn entropy(&mut self, bytes: &mut [u8]) -> Result<(), UuidError> {
        let snapshot = self
            .entropy
            .pop_front()
            .unwrap_or(Err(UuidErrorKind::EntropyUnavailable))
            .map_err(error)?;
        if snapshot.len() != bytes.len() {
            return Err(error(UuidErrorKind::ProviderMisconfigured));
        }
        bytes.copy_from_slice(&snapshot);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_control::{EnvelopeHandle, EnvelopeLimits, ExecutionPhase};
    use crate::test_limits::LimitProfile;
    use std::time::Duration;

    #[test]
    fn uuid_clock_checks_fractional_pre_epoch_and_48_bit_boundaries() {
        // 100 ns is representable by both POSIX and Windows SystemTime.
        for nanos in [100, 999_900, 1_000_000] {
            assert_eq!(
                unix_milliseconds(UNIX_EPOCH - Duration::from_nanos(nanos)),
                Err(error(UuidErrorKind::TimestampOutOfRange))
            );
        }
        assert_eq!(unix_milliseconds(UNIX_EPOCH), Ok(0));
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH + Duration::from_nanos(999_999)),
            Ok(0)
        );
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH + Duration::from_millis(1)),
            Ok(1)
        );
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH + Duration::from_millis(MAX_UNIX_MILLISECONDS as u64)),
            Ok(MAX_UNIX_MILLISECONDS)
        );
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH + Duration::from_millis(MAX_UNIX_MILLISECONDS as u64 + 1)),
            Err(error(UuidErrorKind::TimestampOutOfRange))
        );
    }

    #[test]
    fn uuid_os_errors_have_closed_nominal_kinds_and_no_platform_details() {
        assert_eq!(
            entropy_error(getrandom::Error::UNSUPPORTED),
            error(UuidErrorKind::EntropyUnavailable)
        );
        for failure in [
            getrandom::Error::ERRNO_NOT_POSITIVE,
            getrandom::Error::UNEXPECTED,
            getrandom::Error::new_custom(72),
        ] {
            assert_eq!(entropy_error(failure), error(UuidErrorKind::EntropyFailure));
        }
    }

    #[test]
    fn uuid_fixture_construction_enforces_finite_rows_and_closed_failure_tags() {
        for fixture in [
            UuidTestProviders::new(vec![Ok(UNIX_EPOCH); 257], vec![]),
            UuidTestProviders::new(vec![], vec![Ok(vec![0; 16]); 257]),
        ] {
            assert_eq!(fixture.unwrap_err(), error(UuidErrorKind::ResourceLimit));
        }
        for fixture in [
            UuidTestProviders::new(vec![], vec![Ok(vec![0; 17])]),
            UuidTestProviders::new(vec![Err(UuidErrorKind::EntropyFailure)], vec![]),
            UuidTestProviders::new(vec![], vec![Err(UuidErrorKind::ClockFailure)]),
        ] {
            assert_eq!(
                fixture.unwrap_err(),
                error(UuidErrorKind::ProviderMisconfigured)
            );
        }
        assert!(
            UuidTestProviders::new(vec![Ok(UNIX_EPOCH); 256], vec![Ok(vec![0; 16]); 256]).is_ok()
        );
    }

    #[test]
    fn uuid_fixture_reads_are_once_only_and_exhaustion_never_uses_the_os() {
        let mut provider = UuidTestProviders::new(
            vec![Ok(UNIX_EPOCH), Err(UuidErrorKind::ClockFailure)],
            vec![
                Ok(vec![7; 16]),
                Err(UuidErrorKind::EntropyFailure),
                Ok(vec![1; 9]),
            ],
        )
        .unwrap();
        assert_eq!(provider.clock(), Ok(UNIX_EPOCH));
        assert_eq!(provider.clock(), Err(error(UuidErrorKind::ClockFailure)));
        assert_eq!(
            provider.clock(),
            Err(error(UuidErrorKind::ClockUnavailable))
        );
        let mut bytes = [0; 16];
        provider.entropy(&mut bytes).unwrap();
        assert_eq!(bytes, [7; 16]);
        for kind in [
            UuidErrorKind::EntropyFailure,
            UuidErrorKind::ProviderMisconfigured,
            UuidErrorKind::EntropyUnavailable,
        ] {
            assert_eq!(provider.entropy(&mut bytes), Err(error(kind)));
            assert_eq!(bytes, [7; 16]);
        }
    }

    #[test]
    fn uuid_envelope_shares_once_consumption_and_refuses_replacement_or_closed_calls() {
        let envelope = EnvelopeHandle::new("uuid", EnvelopeLimits::new(1024, 1024, 1024));
        envelope
            .with_uuid_providers(
                UuidTestProviders::new(vec![], vec![Ok(vec![1; 16]), Ok(vec![2; 16])]).unwrap(),
            )
            .unwrap();
        assert_eq!(
            envelope.with_uuid_providers(UuidTestProviders::new(vec![], vec![]).unwrap()),
            Err(error(UuidErrorKind::ProviderMisconfigured))
        );
        envelope.set_phase(ExecutionPhase::Body).unwrap();
        let other = envelope.clone();
        let mut bytes = [0; 16];
        assert_eq!(envelope.uuid_entropy(&mut bytes), Ok(Some(Ok(()))));
        assert_eq!(bytes, [1; 16]);
        assert_eq!(other.uuid_entropy(&mut bytes), Ok(Some(Ok(()))));
        assert_eq!(bytes, [2; 16]);
        assert_eq!(
            other.uuid_entropy(&mut bytes),
            Ok(Some(Err(error(UuidErrorKind::EntropyUnavailable))))
        );
        envelope.close().unwrap();
        assert_eq!(
            other.uuid_entropy(&mut bytes),
            Err(error(UuidErrorKind::ProviderMisconfigured))
        );
        assert_eq!(
            other.uuid_clock(),
            Err(error(UuidErrorKind::ProviderMisconfigured))
        );
        assert_eq!(
            other.with_uuid_providers(UuidTestProviders::new(vec![], vec![]).unwrap()),
            Err(error(UuidErrorKind::ProviderMisconfigured))
        );
    }

    #[test]
    fn uuid_fixture_install_and_phase_budgets_are_atomic() {
        let providers =
            UuidTestProviders::new(vec![Ok(UNIX_EPOCH)], vec![Ok(vec![0; 16])]).unwrap();
        let bytes = providers.logical_bytes();
        let too_small = EnvelopeHandle::new(
            "small",
            EnvelopeLimits::from_profile(LimitProfile::DEFAULT.with_memory(bytes - 1)),
        );
        assert_eq!(
            too_small.with_uuid_providers(providers.clone()),
            Err(error(UuidErrorKind::ResourceLimit))
        );
        assert_eq!(too_small.uuid_clock(), Ok(None));
        let exact = EnvelopeHandle::new(
            "exact",
            EnvelopeLimits::from_profile(LimitProfile::DEFAULT.with_memory(bytes)),
        );
        exact.with_uuid_providers(providers).unwrap();
        exact.set_phase(ExecutionPhase::Body).unwrap();
        assert_eq!(exact.uuid_clock(), Ok(Some(Ok(UNIX_EPOCH))));
        exact.set_phase(ExecutionPhase::Cleanup).unwrap();
        let mut entropy = [0; 16];
        assert_eq!(exact.uuid_entropy(&mut entropy), Ok(Some(Ok(()))));
        exact.set_phase(ExecutionPhase::Closed).unwrap();
        assert_eq!(
            exact.uuid_clock(),
            Err(error(UuidErrorKind::ProviderMisconfigured))
        );
    }

    #[test]
    fn uuid_provider_work_budget_refuses_before_consuming_the_next_snapshot() {
        let envelope = EnvelopeHandle::new(
            "limited",
            EnvelopeLimits::new(1024, 1024, 1024).with_execution_limits(1, 1),
        );
        envelope
            .with_uuid_providers(UuidTestProviders::new(vec![Ok(UNIX_EPOCH)], vec![]).unwrap())
            .unwrap();
        assert_eq!(
            envelope.uuid_clock(),
            Err(error(UuidErrorKind::ResourceLimit))
        );
        envelope.set_phase(ExecutionPhase::Body).unwrap();
        assert_eq!(envelope.uuid_clock(), Ok(Some(Ok(UNIX_EPOCH))));
        assert_eq!(
            envelope.uuid_clock(),
            Err(error(UuidErrorKind::ResourceLimit))
        );
    }
}
