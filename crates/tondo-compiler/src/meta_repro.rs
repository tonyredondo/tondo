//! Reproducibility evidence for hermetic compile-time execution.
//!
//! The runner that performs each variation records only the resulting bytes or
//! canonical diagnostic bytes. This verifier rejects incomplete matrices and
//! any byte drift, making the build reproducibility claim executable.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetaReproDimension {
    WorkingDirectory,
    Environment,
    FilesystemOrder,
    CoreCount,
    Scheduling,
    RepeatedBuild,
}

impl MetaReproDimension {
    pub const ALL: [Self; 6] = [
        Self::WorkingDirectory,
        Self::Environment,
        Self::FilesystemOrder,
        Self::CoreCount,
        Self::Scheduling,
        Self::RepeatedBuild,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkingDirectory => "working-directory",
            Self::Environment => "environment",
            Self::FilesystemOrder => "filesystem-order",
            Self::CoreCount => "core-count",
            Self::Scheduling => "scheduling",
            Self::RepeatedBuild => "repeated-build",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaObservationKind {
    Output,
    Diagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaReproObservation {
    dimension: MetaReproDimension,
    variation: String,
    kind: MetaObservationKind,
    bytes: Vec<u8>,
}

impl MetaReproObservation {
    pub fn new(
        dimension: MetaReproDimension,
        variation: impl Into<String>,
        kind: MetaObservationKind,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, MetaReproError> {
        let variation = variation.into();
        if variation.is_empty() || variation.chars().any(char::is_control) {
            return Err(MetaReproError::InvalidVariation);
        }
        Ok(Self {
            dimension,
            variation,
            kind,
            bytes: bytes.into(),
        })
    }

    pub fn dimension(&self) -> MetaReproDimension {
        self.dimension
    }

    pub fn variation(&self) -> &str {
        &self.variation
    }

    pub fn kind(&self) -> MetaObservationKind {
        self.kind
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaReproReport {
    canonical_kind: MetaObservationKind,
    canonical_bytes: Vec<u8>,
    variations: BTreeMap<MetaReproDimension, Vec<String>>,
}

impl MetaReproReport {
    pub fn canonical_kind(&self) -> MetaObservationKind {
        self.canonical_kind
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn variations(&self, dimension: MetaReproDimension) -> &[String] {
        self.variations
            .get(&dimension)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

pub fn verify_meta_reproducibility(
    observations: impl IntoIterator<Item = MetaReproObservation>,
) -> Result<MetaReproReport, MetaReproError> {
    let mut observations = observations.into_iter().collect::<Vec<_>>();
    observations.sort_by(|left, right| {
        (left.dimension, &left.variation).cmp(&(right.dimension, &right.variation))
    });
    let Some(reference) = observations.first() else {
        return Err(MetaReproError::Empty);
    };
    let mut covered = BTreeSet::new();
    let mut unique = BTreeSet::new();
    let mut variations = BTreeMap::<MetaReproDimension, Vec<String>>::new();
    for observation in &observations {
        if !unique.insert((observation.dimension, observation.variation.clone())) {
            return Err(MetaReproError::DuplicateVariation {
                dimension: observation.dimension,
                variation: observation.variation.clone(),
            });
        }
        covered.insert(observation.dimension);
        variations
            .entry(observation.dimension)
            .or_default()
            .push(observation.variation.clone());
        if observation.kind != reference.kind || observation.bytes != reference.bytes {
            return Err(MetaReproError::Drift {
                dimension: observation.dimension,
                variation: observation.variation.clone(),
            });
        }
    }
    for dimension in MetaReproDimension::ALL {
        if !covered.contains(&dimension) {
            return Err(MetaReproError::MissingDimension(dimension));
        }
    }
    Ok(MetaReproReport {
        canonical_kind: reference.kind,
        canonical_bytes: reference.bytes.clone(),
        variations,
    })
}

/// The closed ambient surface that the `meta` target must deny. `Duration` and
/// pure collection operations are deliberately absent because they require no
/// host observation.
pub const DENIED_META_CAPABILITIES: [&str; 10] = [
    "filesystem",
    "network",
    "process",
    "clock",
    "entropy",
    "threads",
    "async",
    "ffi",
    "unsafe",
    "host-identity",
];

pub fn is_denied_meta_capability(capability: &str) -> bool {
    DENIED_META_CAPABILITIES.contains(&capability)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaReproError {
    Empty,
    InvalidVariation,
    DuplicateVariation {
        dimension: MetaReproDimension,
        variation: String,
    },
    MissingDimension(MetaReproDimension),
    Drift {
        dimension: MetaReproDimension,
        variation: String,
    },
}

impl fmt::Display for MetaReproError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("meta reproducibility matrix is empty"),
            Self::InvalidVariation => formatter.write_str("invalid reproducibility variation"),
            Self::DuplicateVariation {
                dimension,
                variation,
            } => write!(
                formatter,
                "duplicate {} variation `{variation}`",
                dimension.as_str()
            ),
            Self::MissingDimension(dimension) => {
                write!(formatter, "missing {} variation", dimension.as_str())
            }
            Self::Drift {
                dimension,
                variation,
            } => write!(
                formatter,
                "meta output drift under {} variation `{variation}`",
                dimension.as_str()
            ),
        }
    }
}

impl Error for MetaReproError {}

#[cfg(test)]
mod tests {
    use tondo_vm::runtime::{RuntimeValue, VmOutcome};

    use super::*;
    use crate::meta::MetaLimits;
    use crate::meta_test_support::string_artifact;
    use crate::meta_vm::MetaVmLimits;

    fn observations(bytes: &[u8]) -> Vec<MetaReproObservation> {
        MetaReproDimension::ALL
            .into_iter()
            .flat_map(|dimension| {
                ["baseline", "perturbed"].map(move |variation| {
                    MetaReproObservation::new(
                        dimension,
                        variation,
                        MetaObservationKind::Output,
                        bytes,
                    )
                    .unwrap()
                })
            })
            .collect()
    }

    #[test]
    fn complete_matrix_accepts_only_byte_identical_outputs() {
        let report = verify_meta_reproducibility(observations(b"canonical\n")).unwrap();
        assert_eq!(report.canonical_kind(), MetaObservationKind::Output);
        assert_eq!(report.canonical_bytes(), b"canonical\n");
        for dimension in MetaReproDimension::ALL {
            assert_eq!(report.variations(dimension), ["baseline", "perturbed"]);
        }
    }

    #[test]
    fn fresh_meta_vms_produce_identical_bytes_for_every_variation() {
        let artifact = string_artifact("generated");
        let limits = MetaVmLimits::for_request(MetaLimits::new(10_000, 64_000, 64).unwrap());
        let mut matrix = Vec::new();
        for dimension in MetaReproDimension::ALL {
            for variation in ["baseline", "perturbed"] {
                let execution = artifact.clone().load(limits).unwrap().run().unwrap();
                let VmOutcome::Returned(RuntimeValue::String(value)) = execution.outcome else {
                    panic!("string artifact must return a string");
                };
                matrix.push(
                    MetaReproObservation::new(
                        dimension,
                        variation,
                        MetaObservationKind::Output,
                        value.into_bytes(),
                    )
                    .unwrap(),
                );
            }
        }
        assert_eq!(
            verify_meta_reproducibility(matrix)
                .unwrap()
                .canonical_bytes(),
            b"generated"
        );
    }

    #[test]
    fn incomplete_duplicate_and_drifting_matrices_fail_closed() {
        assert_eq!(
            verify_meta_reproducibility(Vec::new()),
            Err(MetaReproError::Empty)
        );
        let mut incomplete = observations(b"same");
        incomplete.retain(|item| item.dimension() != MetaReproDimension::CoreCount);
        assert_eq!(
            verify_meta_reproducibility(incomplete),
            Err(MetaReproError::MissingDimension(
                MetaReproDimension::CoreCount
            ))
        );
        let mut duplicate = observations(b"same");
        duplicate.push(duplicate[0].clone());
        assert!(matches!(
            verify_meta_reproducibility(duplicate),
            Err(MetaReproError::DuplicateVariation { .. })
        ));
        let mut drift = observations(b"same");
        let index = drift
            .iter()
            .position(|item| item.dimension() == MetaReproDimension::Scheduling)
            .unwrap();
        drift[index] = MetaReproObservation::new(
            MetaReproDimension::Scheduling,
            drift[index].variation(),
            MetaObservationKind::Diagnostics,
            b"different",
        )
        .unwrap();
        assert!(matches!(
            verify_meta_reproducibility(drift),
            Err(MetaReproError::Drift {
                dimension: MetaReproDimension::Scheduling,
                ..
            })
        ));
    }

    #[test]
    fn denied_capability_catalog_is_closed_and_exact() {
        assert_eq!(DENIED_META_CAPABILITIES.len(), 10);
        for capability in DENIED_META_CAPABILITIES {
            assert!(is_denied_meta_capability(capability));
        }
        assert!(!is_denied_meta_capability("duration"));
        assert!(!is_denied_meta_capability("collections"));
    }

    #[test]
    fn observations_reject_unstable_labels_and_expose_values() {
        assert_eq!(
            MetaReproObservation::new(
                MetaReproDimension::Environment,
                "",
                MetaObservationKind::Diagnostics,
                b"error",
            ),
            Err(MetaReproError::InvalidVariation)
        );
        let observation = MetaReproObservation::new(
            MetaReproDimension::Environment,
            "locale=C",
            MetaObservationKind::Diagnostics,
            b"error",
        )
        .unwrap();
        assert_eq!(observation.kind(), MetaObservationKind::Diagnostics);
        assert_eq!(observation.bytes(), b"error");
    }
}
