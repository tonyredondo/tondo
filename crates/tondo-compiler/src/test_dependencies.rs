//! Closed test-only dependency graph.
//!
//! Development dependencies are validated beside, but never merged into, the
//! production `PackageGraph`. This module consumes already supplied lockfile
//! interface records and exposes only pure graph/visibility operations.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::artifact::{sha256, validate_sha256};
use crate::package::{PackageAlias, PackageId};
use crate::project::{BOOTSTRAP_STANDARD_PACKAGE, ProjectPlan};
use crate::test_plan::{TestProjectPlan, TestSourceClass};

pub const TEST_DEPENDENCY_GRAPH_FORMAT: &str = "tondo-test-dependency-graph-draft/1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDependencyRecord {
    package: PackageId,
    interface_path: String,
    interface_sha256: String,
    dependencies: BTreeMap<String, PackageId>,
}

impl TestDependencyRecord {
    pub fn new(
        package: impl Into<String>,
        interface_path: impl Into<String>,
        interface_sha256: impl Into<String>,
        dependencies: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, DependencyGraphError> {
        let package =
            PackageId::new(package.into()).map_err(|error| DependencyGraphError::InvalidField {
                field: "package",
                message: error.to_string(),
            })?;
        let interface_path = canonical_path("interface_path", &interface_path.into())?;
        let interface_sha256 = interface_sha256.into();
        validate_sha256(&interface_sha256).map_err(|error| DependencyGraphError::InvalidField {
            field: "interface_sha256",
            message: error.to_string(),
        })?;
        let mut normalized = BTreeMap::new();
        for (alias, dependency) in dependencies {
            let alias =
                PackageAlias::new(&alias).map_err(|error| DependencyGraphError::InvalidField {
                    field: "dependencies.alias",
                    message: error.to_string(),
                })?;
            let dependency =
                PackageId::new(dependency).map_err(|error| DependencyGraphError::InvalidField {
                    field: "dependencies.package",
                    message: error.to_string(),
                })?;
            if normalized
                .insert(alias.as_str().to_owned(), dependency)
                .is_some()
            {
                return Err(DependencyGraphError::Duplicate {
                    kind: "dependency alias",
                    value: alias.to_string(),
                });
            }
        }
        Ok(Self {
            package,
            interface_path,
            interface_sha256,
            dependencies: normalized,
        })
    }

    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn interface_path(&self) -> &str {
        &self.interface_path
    }

    pub fn interface_sha256(&self) -> &str {
        &self.interface_sha256
    }

    pub fn dependencies(&self) -> &BTreeMap<String, PackageId> {
        &self.dependencies
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDependencyNode {
    package: PackageId,
    interface_path: String,
    interface_sha256: String,
    dependencies: BTreeMap<String, PackageId>,
}

impl TestDependencyNode {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn interface_path(&self) -> &str {
        &self.interface_path
    }

    pub fn interface_sha256(&self) -> &str {
        &self.interface_sha256
    }

    pub fn dependencies(&self) -> &BTreeMap<String, PackageId> {
        &self.dependencies
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDependencyGraph {
    aliases: BTreeMap<String, PackageId>,
    nodes: BTreeMap<PackageId, TestDependencyNode>,
}

impl TestDependencyGraph {
    /// Build from the test plan while keeping production package IDs outside
    /// this graph. The supplied records represent the verified test-interface
    /// entries from the lockfile; no bytes are opened here.
    pub fn from_plan(
        project: &ProjectPlan,
        plan: &TestProjectPlan,
        records: Vec<TestDependencyRecord>,
    ) -> Result<Self, DependencyGraphError> {
        let expected = plan
            .dev_dependencies()
            .iter()
            .map(|dependency| ExpectedDependency {
                alias: dependency.alias().to_owned(),
                package: dependency.package().to_owned(),
                interface_path: dependency.interface_path().to_owned(),
                interface_sha256: dependency.sha256().to_owned(),
            })
            .collect::<Vec<_>>();
        let production = project
            .package_ids()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        build_for(&expected, &production, records)
    }

    pub fn packages(&self) -> impl ExactSizeIterator<Item = &TestDependencyNode> {
        self.nodes.values()
    }

    pub fn package(&self, package: &PackageId) -> Option<&TestDependencyNode> {
        self.nodes.get(package)
    }

    /// Resolve a test alias. Production sources are deliberately rejected
    /// before alias lookup, so no dev edge can leak into the product graph.
    pub fn resolve_alias(
        &self,
        class: TestSourceClass,
        alias: &str,
    ) -> Result<&PackageId, DependencyGraphError> {
        if class == TestSourceClass::Production {
            return Err(DependencyGraphError::DevDependencyNotVisible {
                alias: alias.to_owned(),
            });
        }
        let alias =
            PackageAlias::new(alias).map_err(|error| DependencyGraphError::InvalidField {
                field: "alias",
                message: error.to_string(),
            })?;
        self.aliases
            .get(alias.as_str())
            .ok_or_else(|| DependencyGraphError::UnknownAlias(alias.to_string()))
    }

    pub fn resolve_from(
        &self,
        package: &PackageId,
        alias: &str,
    ) -> Result<&PackageId, DependencyGraphError> {
        let node = self
            .nodes
            .get(package)
            .ok_or_else(|| DependencyGraphError::UnknownPackage(package.to_string()))?;
        let alias =
            PackageAlias::new(alias).map_err(|error| DependencyGraphError::InvalidField {
                field: "alias",
                message: error.to_string(),
            })?;
        node.dependencies
            .get(alias.as_str())
            .ok_or_else(|| DependencyGraphError::UnknownAlias(alias.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyGraphError {
    InvalidField {
        field: &'static str,
        message: String,
    },
    Duplicate {
        kind: &'static str,
        value: String,
    },
    MissingRecord {
        package: String,
    },
    UnexpectedRecord {
        package: String,
    },
    MetadataMismatch {
        package: String,
        field: &'static str,
        expected: String,
        actual: String,
    },
    ProductionOverlap {
        package: String,
    },
    UnknownDependency {
        package: String,
        dependency: String,
    },
    DependencyCycle,
    DevDependencyNotVisible {
        alias: String,
    },
    UnknownAlias(String),
    UnknownPackage(String),
}

impl fmt::Display for DependencyGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::Duplicate { kind, value } => write!(formatter, "duplicate {kind} `{value}`"),
            Self::MissingRecord { package } => {
                write!(formatter, "missing test dependency record `{package}`")
            }
            Self::UnexpectedRecord { package } => {
                write!(formatter, "unexpected test dependency record `{package}`")
            }
            Self::MetadataMismatch {
                package,
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "test dependency `{package}` has {field} `{actual}`, expected `{expected}`"
            ),
            Self::ProductionOverlap { package } => write!(
                formatter,
                "test dependency `{package}` belongs to the production graph"
            ),
            Self::UnknownDependency {
                package,
                dependency,
            } => write!(
                formatter,
                "test dependency `{package}` references unknown `{dependency}`"
            ),
            Self::DependencyCycle => formatter.write_str("test dependency graph contains a cycle"),
            Self::DevDependencyNotVisible { alias } => write!(
                formatter,
                "dev-dependency alias `{alias}` is not visible to production"
            ),
            Self::UnknownAlias(alias) => {
                write!(formatter, "unknown test dependency alias `{alias}`")
            }
            Self::UnknownPackage(package) => {
                write!(formatter, "unknown test dependency package `{package}`")
            }
        }
    }
}

impl Error for DependencyGraphError {}

/// Fingerprint of production-only inputs. Test plans and dev records are not
/// accepted by this function, making the non-interference boundary explicit.
pub fn production_identity(project: &ProjectPlan) -> String {
    let mut bytes = Vec::new();
    append_field(&mut bytes, project.manifest_hash());
    append_field(&mut bytes, project.lockfile_hash());
    append_field(&mut bytes, project.target_name());
    append_field(&mut bytes, project.profile().as_str());
    for package in project.package_ids() {
        append_field(&mut bytes, package);
    }
    for source in project.selected_source_paths() {
        append_field(&mut bytes, source);
    }
    for capability in project.capabilities() {
        append_field(&mut bytes, capability.as_str());
    }
    for feature in project.features() {
        append_field(&mut bytes, feature.as_str());
    }
    sha256(&bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedDependency {
    alias: String,
    package: String,
    interface_path: String,
    interface_sha256: String,
}

fn build_for(
    expected: &[ExpectedDependency],
    production: &BTreeSet<String>,
    records: Vec<TestDependencyRecord>,
) -> Result<TestDependencyGraph, DependencyGraphError> {
    let mut record_map = BTreeMap::new();
    for record in records {
        let package = record.package.to_string();
        if record_map.insert(package.clone(), record).is_some() {
            return Err(DependencyGraphError::Duplicate {
                kind: "test dependency package",
                value: package,
            });
        }
    }
    let expected_packages = expected
        .iter()
        .map(|dependency| dependency.package.clone())
        .collect::<BTreeSet<_>>();
    let mut aliases = BTreeMap::new();
    for dependency in expected {
        let alias = PackageAlias::new(&dependency.alias).map_err(|error| {
            DependencyGraphError::InvalidField {
                field: "dev_dependencies.alias",
                message: error.to_string(),
            }
        })?;
        let package = PackageId::new(dependency.package.clone()).map_err(|error| {
            DependencyGraphError::InvalidField {
                field: "dev_dependencies.package",
                message: error.to_string(),
            }
        })?;
        if production.contains(package.as_str()) {
            return Err(DependencyGraphError::ProductionOverlap {
                package: package.to_string(),
            });
        }
        if aliases.insert(alias.to_string(), package.clone()).is_some() {
            return Err(DependencyGraphError::Duplicate {
                kind: "test dependency alias",
                value: alias.to_string(),
            });
        }
        let Some(record) = record_map.get(package.as_str()) else {
            return Err(DependencyGraphError::MissingRecord {
                package: package.to_string(),
            });
        };
        if record.interface_path != dependency.interface_path {
            return Err(DependencyGraphError::MetadataMismatch {
                package: package.to_string(),
                field: "interface_path",
                expected: dependency.interface_path.clone(),
                actual: record.interface_path.clone(),
            });
        }
        if record.interface_sha256 != dependency.interface_sha256 {
            return Err(DependencyGraphError::MetadataMismatch {
                package: package.to_string(),
                field: "interface_sha256",
                expected: dependency.interface_sha256.clone(),
                actual: record.interface_sha256.clone(),
            });
        }
    }
    for package in record_map.keys() {
        if !expected_packages.contains(package) {
            return Err(DependencyGraphError::UnexpectedRecord {
                package: package.clone(),
            });
        }
    }

    for (package, record) in &record_map {
        for dependency in record.dependencies.values() {
            if dependency.as_str() != BOOTSTRAP_STANDARD_PACKAGE
                && !record_map.contains_key(dependency.as_str())
            {
                if production.contains(dependency.as_str()) {
                    return Err(DependencyGraphError::ProductionOverlap {
                        package: dependency.to_string(),
                    });
                }
                return Err(DependencyGraphError::UnknownDependency {
                    package: package.clone(),
                    dependency: dependency.to_string(),
                });
            }
        }
    }
    if has_cycle(&record_map) {
        return Err(DependencyGraphError::DependencyCycle);
    }
    let nodes = record_map
        .into_iter()
        .map(|(package, record)| {
            (
                PackageId::new(package).expect("record map keys are validated package IDs"),
                TestDependencyNode {
                    package: record.package,
                    interface_path: record.interface_path,
                    interface_sha256: record.interface_sha256,
                    dependencies: record.dependencies,
                },
            )
        })
        .collect();
    Ok(TestDependencyGraph { aliases, nodes })
}

fn has_cycle(records: &BTreeMap<String, TestDependencyRecord>) -> bool {
    fn visit(
        package: &str,
        records: &BTreeMap<String, TestDependencyRecord>,
        visiting: &mut BTreeSet<String>,
        complete: &mut BTreeSet<String>,
    ) -> bool {
        if complete.contains(package) {
            return false;
        }
        if !visiting.insert(package.to_owned()) {
            return true;
        }
        if let Some(record) = records.get(package) {
            for dependency in record.dependencies.values() {
                if dependency.as_str() == BOOTSTRAP_STANDARD_PACKAGE {
                    continue;
                }
                if visit(dependency.as_str(), records, visiting, complete) {
                    return true;
                }
            }
        }
        visiting.remove(package);
        complete.insert(package.to_owned());
        false
    }

    let mut visiting = BTreeSet::new();
    let mut complete = BTreeSet::new();
    records
        .keys()
        .any(|package| visit(package, records, &mut visiting, &mut complete))
}

fn canonical_path(field: &'static str, value: &str) -> Result<String, DependencyGraphError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', '\n', '\r'])
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(DependencyGraphError::InvalidField {
            field,
            message: "path must be relative, slash-separated and canonical".into(),
        });
    }
    Ok(value.to_owned())
}

fn append_field(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(value.len().to_string().as_bytes());
    bytes.push(b':');
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(b'|');
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn expected(alias: &str, package: &str, path: &str, sha: &str) -> ExpectedDependency {
        ExpectedDependency {
            alias: alias.into(),
            package: package.into(),
            interface_path: path.into(),
            interface_sha256: sha.into(),
        }
    }

    fn record(
        package: &str,
        path: &str,
        sha: &str,
        dependencies: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> TestDependencyRecord {
        TestDependencyRecord::new(
            package,
            path,
            sha,
            dependencies
                .into_iter()
                .map(|(alias, package)| (alias.into(), package.into())),
        )
        .unwrap()
    }

    #[test]
    fn records_normalize_and_validate_metadata() {
        let value = record(
            "registry:test@1#abc",
            "interfaces/test.ti",
            SHA_A,
            [("helper", "toolchain:std:0.1-bootstrap")],
        );
        assert_eq!(value.package().as_str(), "registry:test@1#abc");
        assert_eq!(value.dependencies().len(), 1);
        assert!(TestDependencyRecord::new("pkg", "../bad", SHA_A, []).is_err());
        assert!(TestDependencyRecord::new("pkg", "iface", "bad", []).is_err());
    }

    #[test]
    fn isolated_graph_accepts_records_in_any_order_and_exposes_aliases() {
        let expected = vec![
            expected(
                "testing",
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
            ),
            expected(
                "support",
                "registry:support@1#def",
                "interfaces/support.ti",
                SHA_B,
            ),
        ];
        let graph = build_for(
            &expected,
            &BTreeSet::from(["workspace:app@1".into()]),
            vec![
                record("registry:support@1#def", "interfaces/support.ti", SHA_B, []),
                record(
                    "registry:test@1#abc",
                    "interfaces/test.ti",
                    SHA_A,
                    [("support", "registry:support@1#def")],
                ),
            ],
        )
        .unwrap();
        assert_eq!(graph.packages().len(), 2);
        assert_eq!(
            graph
                .resolve_alias(TestSourceClass::UnitTest, "testing")
                .unwrap()
                .as_str(),
            "registry:test@1#abc"
        );
        assert_eq!(
            graph
                .resolve_from(&PackageId::new("registry:test@1#abc").unwrap(), "support")
                .unwrap()
                .as_str(),
            "registry:support@1#def"
        );
    }

    #[test]
    fn production_sources_cannot_resolve_test_aliases() {
        let graph = build_for(
            &[expected(
                "testing",
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::new(),
            vec![record(
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
                [],
            )],
        )
        .unwrap();
        assert!(matches!(
            graph.resolve_alias(TestSourceClass::Production, "testing"),
            Err(DependencyGraphError::DevDependencyNotVisible { .. })
        ));
        assert!(
            graph
                .resolve_alias(TestSourceClass::IntegrationTest, "testing")
                .is_ok()
        );
    }

    #[test]
    fn metadata_and_record_sets_must_match_the_closed_plan() {
        let expected = [expected(
            "testing",
            "registry:test@1#abc",
            "interfaces/test.ti",
            SHA_A,
        )];
        let error = build_for(
            &expected,
            &BTreeSet::new(),
            vec![record(
                "registry:test@1#abc",
                "interfaces/other.ti",
                SHA_A,
                [],
            )],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DependencyGraphError::MetadataMismatch { .. }
        ));
        let error = build_for(
            &expected,
            &BTreeSet::new(),
            vec![record(
                "registry:other@1#abc",
                "interfaces/test.ti",
                SHA_A,
                [],
            )],
        )
        .unwrap_err();
        assert!(matches!(error, DependencyGraphError::MissingRecord { .. }));
    }

    #[test]
    fn production_overlap_and_unexpected_records_are_rejected() {
        let error = build_for(
            &[expected(
                "testing",
                "workspace:app@1",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::from(["workspace:app@1".into()]),
            vec![record("workspace:app@1", "interfaces/test.ti", SHA_A, [])],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DependencyGraphError::ProductionOverlap { .. }
        ));
        let error = build_for(
            &[expected(
                "testing",
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::new(),
            vec![
                record("registry:test@1#abc", "interfaces/test.ti", SHA_A, []),
                record("registry:extra@1#abc", "interfaces/extra.ti", SHA_B, []),
            ],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DependencyGraphError::UnexpectedRecord { .. }
        ));
    }

    #[test]
    fn transitive_edges_are_closed_to_dev_nodes_or_standard() {
        let accepted = build_for(
            &[expected(
                "testing",
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::new(),
            vec![record(
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
                [("stdlib", BOOTSTRAP_STANDARD_PACKAGE)],
            )],
        )
        .unwrap();
        assert_eq!(accepted.packages().count(), 1);
        let error = build_for(
            &[expected(
                "testing",
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::new(),
            vec![record(
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
                [("prod", "workspace:app@1")],
            )],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DependencyGraphError::UnknownDependency { .. }
        ));
    }

    #[test]
    fn cycles_are_rejected_before_graph_materialization() {
        let error = build_for(
            &[
                expected("a", "registry:a@1", "interfaces/a.ti", SHA_A),
                expected("b", "registry:b@1", "interfaces/b.ti", SHA_B),
            ],
            &BTreeSet::new(),
            vec![
                record(
                    "registry:a@1",
                    "interfaces/a.ti",
                    SHA_A,
                    [("b", "registry:b@1")],
                ),
                record(
                    "registry:b@1",
                    "interfaces/b.ti",
                    SHA_B,
                    [("a", "registry:a@1")],
                ),
            ],
        )
        .unwrap_err();
        assert!(matches!(error, DependencyGraphError::DependencyCycle));
    }

    #[test]
    fn duplicate_aliases_and_unknown_aliases_remain_errors() {
        let error = TestDependencyRecord::new(
            "registry:test@1",
            "interfaces/test.ti",
            SHA_A,
            [
                ("helper".into(), "pkg:a".into()),
                ("helper".into(), "pkg:b".into()),
            ],
        )
        .unwrap_err();
        assert!(matches!(error, DependencyGraphError::Duplicate { .. }));
        let graph = build_for(
            &[expected(
                "testing",
                "registry:test@1",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::new(),
            vec![record("registry:test@1", "interfaces/test.ti", SHA_A, [])],
        )
        .unwrap();
        assert!(matches!(
            graph.resolve_alias(TestSourceClass::UnitTest, "unknown"),
            Err(DependencyGraphError::UnknownAlias(_))
        ));
    }

    #[test]
    fn production_identity_is_framed_and_does_not_depend_on_test_records() {
        let mut bytes_a = Vec::new();
        append_field(&mut bytes_a, "manifest");
        append_field(&mut bytes_a, "lockfile");
        let mut bytes_b = bytes_a.clone();
        append_field(&mut bytes_b, "test-dependency-a");
        assert_ne!(sha256(&bytes_a), sha256(&bytes_b));
        assert_eq!(
            TEST_DEPENDENCY_GRAPH_FORMAT,
            "tondo-test-dependency-graph-draft/1"
        );
    }

    #[test]
    fn dependency_accessors_resolution_edges_and_error_messages_are_closed() {
        let value = record(
            "registry:test@1#abc",
            "interfaces/test.ti",
            SHA_A,
            [("support", "registry:support@1#def")],
        );
        assert_eq!(value.package().as_str(), "registry:test@1#abc");
        assert_eq!(value.interface_path(), "interfaces/test.ti");
        assert_eq!(value.interface_sha256(), SHA_A);
        assert_eq!(
            value.dependencies()["support"].as_str(),
            "registry:support@1#def"
        );
        assert!(TestDependencyRecord::new("pkg", "", SHA_A, []).is_err());
        assert!(
            TestDependencyRecord::new(
                "pkg",
                "iface",
                SHA_A,
                [("bad alias".into(), "pkg:b".into())]
            )
            .is_err()
        );
        assert!(
            TestDependencyRecord::new("pkg", "iface", SHA_A, [("a".into(), "".into())]).is_err()
        );

        let graph = build_for(
            &[expected(
                "testing",
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
            )],
            &BTreeSet::new(),
            vec![record(
                "registry:test@1#abc",
                "interfaces/test.ti",
                SHA_A,
                [],
            )],
        )
        .unwrap();
        let package = PackageId::new("registry:test@1#abc").unwrap();
        let node = graph.package(&package).unwrap();
        assert_eq!(node.package().as_str(), package.as_str());
        assert_eq!(node.interface_path(), "interfaces/test.ti");
        assert_eq!(node.interface_sha256(), SHA_A);
        assert!(node.dependencies().is_empty());
        assert!(
            graph
                .package(&PackageId::new("registry:missing@1").unwrap())
                .is_none()
        );
        assert!(matches!(
            graph.resolve_from(&PackageId::new("registry:missing@1").unwrap(), "x"),
            Err(DependencyGraphError::UnknownPackage(_))
        ));
        assert!(matches!(
            graph.resolve_from(&package, "bad alias"),
            Err(DependencyGraphError::InvalidField { .. })
        ));
        assert!(matches!(
            graph.resolve_from(&package, "missing"),
            Err(DependencyGraphError::UnknownAlias(_))
        ));
        assert!(matches!(
            graph.resolve_alias(TestSourceClass::UnitTest, "bad alias"),
            Err(DependencyGraphError::InvalidField { .. })
        ));

        let duplicate_record = record("registry:test@1#abc", "interfaces/test.ti", SHA_A, []);
        assert!(matches!(
            build_for(
                &[expected(
                    "testing",
                    "registry:test@1#abc",
                    "interfaces/test.ti",
                    SHA_A
                )],
                &BTreeSet::new(),
                vec![duplicate_record.clone(), duplicate_record],
            ),
            Err(DependencyGraphError::Duplicate {
                kind: "test dependency package",
                ..
            })
        ));
        assert!(matches!(
            build_for(
                &[
                    expected(
                        "testing",
                        "registry:test@1#abc",
                        "interfaces/test.ti",
                        SHA_A
                    ),
                    expected("testing", "registry:other@1", "interfaces/other.ti", SHA_B)
                ],
                &BTreeSet::new(),
                vec![
                    record("registry:test@1#abc", "interfaces/test.ti", SHA_A, []),
                    record("registry:other@1", "interfaces/other.ti", SHA_B, [])
                ],
            ),
            Err(DependencyGraphError::Duplicate {
                kind: "test dependency alias",
                ..
            })
        ));
        assert!(matches!(
            build_for(
                &[expected(
                    "testing",
                    "registry:test@1#abc",
                    "interfaces/test.ti",
                    SHA_A
                )],
                &BTreeSet::new(),
                vec![record(
                    "registry:test@1#abc",
                    "interfaces/test.ti",
                    SHA_B,
                    []
                )],
            ),
            Err(DependencyGraphError::MetadataMismatch {
                field: "interface_sha256",
                ..
            })
        ));
        assert!(matches!(
            build_for(
                &[expected(
                    "testing",
                    "registry:test@1#abc",
                    "interfaces/test.ti",
                    SHA_A
                )],
                &BTreeSet::from(["workspace:app@1".into()]),
                vec![record(
                    "registry:test@1#abc",
                    "interfaces/test.ti",
                    SHA_A,
                    [("app", "workspace:app@1")]
                )],
            ),
            Err(DependencyGraphError::ProductionOverlap { .. })
        ));

        for error in [
            DependencyGraphError::InvalidField {
                field: "x",
                message: "bad".into(),
            },
            DependencyGraphError::Duplicate {
                kind: "x",
                value: "v".into(),
            },
            DependencyGraphError::MissingRecord {
                package: "p".into(),
            },
            DependencyGraphError::UnexpectedRecord {
                package: "p".into(),
            },
            DependencyGraphError::MetadataMismatch {
                package: "p".into(),
                field: "x",
                expected: "a".into(),
                actual: "b".into(),
            },
            DependencyGraphError::ProductionOverlap {
                package: "p".into(),
            },
            DependencyGraphError::UnknownDependency {
                package: "p".into(),
                dependency: "d".into(),
            },
            DependencyGraphError::DependencyCycle,
            DependencyGraphError::DevDependencyNotVisible { alias: "a".into() },
            DependencyGraphError::UnknownAlias("a".into()),
            DependencyGraphError::UnknownPackage("p".into()),
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
