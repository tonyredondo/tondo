//! Deterministic, host-independent test source discovery.
//!
//! The host may enumerate candidate files, but this module owns the semantic
//! classification and plan reconciliation. It never opens paths, follows a
//! symlink or infers a root from a common prefix.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use crate::source::ModulePath;
use crate::test_plan::{TestProjectPlan, TestSourceClass, TestSourceRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryEntry {
    physical_path: String,
    logical_path: String,
    module: String,
    regular_file: bool,
    symlink_escape: bool,
}

impl DiscoveryEntry {
    pub fn new(
        physical_path: impl Into<String>,
        logical_path: impl Into<String>,
        module: impl Into<String>,
    ) -> Self {
        Self {
            physical_path: physical_path.into(),
            logical_path: logical_path.into(),
            module: module.into(),
            regular_file: true,
            symlink_escape: false,
        }
    }

    pub fn with_file_state(mut self, regular_file: bool, symlink_escape: bool) -> Self {
        self.regular_file = regular_file;
        self.symlink_escape = symlink_escape;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRoot {
    class: TestSourceClass,
    physical_path: String,
    logical_path: String,
}

impl DiscoveryRoot {
    pub fn new(
        class: TestSourceClass,
        physical_path: impl Into<String>,
        logical_path: impl Into<String>,
    ) -> Result<Self, DiscoveryError> {
        let physical_path = canonical_path("root.physical_path", physical_path.into())?;
        let logical_path = canonical_path("root.logical_path", logical_path.into())?;
        Ok(Self {
            class,
            physical_path,
            logical_path,
        })
    }

    pub fn class(&self) -> TestSourceClass {
        self.class
    }

    pub fn physical_path(&self) -> &str {
        &self.physical_path
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryConfig {
    repository_root: String,
    roots: Vec<DiscoveryRoot>,
}

impl DiscoveryConfig {
    pub fn new(
        repository_root: impl Into<String>,
        roots: Vec<DiscoveryRoot>,
    ) -> Result<Self, DiscoveryError> {
        let repository_root = canonical_path("repository_root", repository_root.into())?;
        if roots.is_empty() {
            return Err(DiscoveryError::InvalidField {
                field: "roots",
                message: "at least one root is required".into(),
            });
        }
        let mut roots = roots;
        roots.sort_by(|left, right| {
            (
                left.class,
                left.logical_path.as_str(),
                left.physical_path.as_str(),
            )
                .cmp(&(
                    right.class,
                    right.logical_path.as_str(),
                    right.physical_path.as_str(),
                ))
        });
        let mut identities = BTreeSet::new();
        for root in &roots {
            if !identities.insert((
                root.class,
                root.physical_path.clone(),
                root.logical_path.clone(),
            )) {
                return Err(DiscoveryError::Duplicate {
                    kind: "root",
                    value: root.physical_path.clone(),
                });
            }
        }
        Ok(Self {
            repository_root,
            roots,
        })
    }

    pub fn from_plan(plan: &TestProjectPlan) -> Result<Self, DiscoveryError> {
        let roots = plan
            .roots()
            .iter()
            .map(root_from_plan)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(plan.repository_root(), roots)
    }

    pub fn repository_root(&self) -> &str {
        &self.repository_root
    }

    pub fn roots(&self) -> &[DiscoveryRoot] {
        &self.roots
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSource {
    class: TestSourceClass,
    physical_path: String,
    logical_path: String,
    module: String,
    input: String,
}

impl DiscoveredSource {
    pub fn class(&self) -> TestSourceClass {
        self.class
    }

    pub fn physical_path(&self) -> &str {
        &self.physical_path
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn input(&self) -> &str {
        &self.input
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSource {
    pub class: TestSourceClass,
    pub physical_path: String,
    pub logical_path: String,
    pub module: String,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    InvalidField {
        field: &'static str,
        message: String,
    },
    Unclassified {
        physical_path: String,
    },
    SymlinkEscape {
        physical_path: String,
    },
    NotRegularFile {
        physical_path: String,
    },
    Duplicate {
        kind: &'static str,
        value: String,
    },
    PlanDrift {
        missing: Vec<String>,
        additional: Vec<String>,
    },
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::Unclassified { physical_path } => {
                write!(
                    formatter,
                    "source `{physical_path}` is not a discoverable test source"
                )
            }
            Self::SymlinkEscape { physical_path } => {
                write!(
                    formatter,
                    "source `{physical_path}` escapes its declared root through a symlink"
                )
            }
            Self::NotRegularFile { physical_path } => {
                write!(formatter, "source `{physical_path}` is not a regular file")
            }
            Self::Duplicate { kind, value } => write!(formatter, "duplicate {kind} `{value}`"),
            Self::PlanDrift {
                missing,
                additional,
            } => write!(
                formatter,
                "discovery differs from the closed plan (missing: {:?}, additional: {:?})",
                missing, additional
            ),
        }
    }
}

impl Error for DiscoveryError {}

pub fn discover(
    config: &DiscoveryConfig,
    mut entries: Vec<DiscoveryEntry>,
) -> Result<Vec<DiscoveredSource>, DiscoveryError> {
    entries.sort_by(|left, right| {
        left.physical_path
            .as_bytes()
            .cmp(right.physical_path.as_bytes())
    });
    let mut physical = BTreeSet::new();
    let mut logical_nodes = BTreeSet::new();
    let mut output = Vec::with_capacity(entries.len());
    for entry in entries {
        let physical_path = canonical_path("entry.physical_path", entry.physical_path)?;
        let logical_path = canonical_path("entry.logical_path", entry.logical_path)?;
        let module =
            ModulePath::new(&entry.module).map_err(|error| DiscoveryError::InvalidField {
                field: "entry.module",
                message: error.to_string(),
            })?;
        if !entry.regular_file {
            return Err(DiscoveryError::NotRegularFile { physical_path });
        }
        if entry.symlink_escape {
            return Err(DiscoveryError::SymlinkEscape { physical_path });
        }
        if !physical.insert(physical_path.clone()) {
            return Err(DiscoveryError::Duplicate {
                kind: "physical source",
                value: physical_path,
            });
        }
        let class = classify(config, &physical_path, &logical_path);
        let Some(class) = class else {
            return Err(DiscoveryError::Unclassified { physical_path });
        };
        let module = module.to_string();
        let node = (class, logical_path.clone(), module.clone());
        if !logical_nodes.insert(node) {
            return Err(DiscoveryError::Duplicate {
                kind: "logical source/module",
                value: format!("{}::{logical_path}::{module}", class.as_str()),
            });
        }
        output.push(DiscoveredSource {
            input: format!("source:{}:{physical_path}", class.as_str()),
            class,
            physical_path,
            logical_path,
            module,
        });
    }
    Ok(output)
}

pub fn reconcile_plan(
    plan: &TestProjectPlan,
    discovered: &[DiscoveredSource],
) -> Result<(), DiscoveryError> {
    let expected = plan
        .sources()
        .iter()
        .map(|source| ExpectedSource {
            class: source.class(),
            physical_path: source.physical_path().to_owned(),
            logical_path: source.logical_path().to_owned(),
            module: source.module().to_owned(),
            input: source.input().to_owned(),
        })
        .collect::<Vec<_>>();
    reconcile_expected(&expected, discovered)
}

pub fn reconcile_expected(
    expected: &[ExpectedSource],
    discovered: &[DiscoveredSource],
) -> Result<(), DiscoveryError> {
    let mut expected_keys = BTreeSet::new();
    for source in expected {
        let key = source_key(source);
        if !expected_keys.insert(key.clone()) {
            return Err(DiscoveryError::Duplicate {
                kind: "expected source",
                value: key,
            });
        }
    }
    let mut actual_keys = BTreeSet::new();
    for source in discovered {
        let key = discovered_key(source);
        if !actual_keys.insert(key.clone()) {
            return Err(DiscoveryError::Duplicate {
                kind: "discovered source",
                value: key,
            });
        }
    }
    let expected = expected_keys;
    let actual = actual_keys;
    if expected == actual {
        return Ok(());
    }
    let missing = expected
        .difference(&actual)
        .map(|key| key.to_string())
        .collect();
    let additional = actual
        .difference(&expected)
        .map(|key| key.to_string())
        .collect();
    Err(DiscoveryError::PlanDrift {
        missing,
        additional,
    })
}

fn classify(
    config: &DiscoveryConfig,
    physical_path: &str,
    logical_path: &str,
) -> Option<TestSourceClass> {
    let under_tests = path_within("tests", physical_path);
    if under_tests {
        return config
            .roots
            .iter()
            .any(|root| {
                root.class == TestSourceClass::IntegrationTest
                    && path_within(&root.physical_path, physical_path)
                    && path_within(&root.logical_path, logical_path)
            })
            .then_some(TestSourceClass::IntegrationTest);
    }
    if physical_path.ends_with("_test.to")
        && config.roots.iter().any(|root| {
            root.class == TestSourceClass::Production
                && path_within(&root.physical_path, physical_path)
                && path_within(&root.logical_path, logical_path)
        })
    {
        return Some(TestSourceClass::UnitTest);
    }
    config
        .roots
        .iter()
        .find(|root| {
            path_within(&root.physical_path, physical_path)
                && path_within(&root.logical_path, logical_path)
        })
        .map(|root| root.class)
}

fn root_from_plan(root: &TestSourceRoot) -> Result<DiscoveryRoot, DiscoveryError> {
    DiscoveryRoot::new(root.class(), root.physical_path(), root.logical_path())
}

fn source_key(source: &ExpectedSource) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        source.class.as_str(),
        source.physical_path,
        source.logical_path,
        source.module,
        source.input
    )
}

fn discovered_key(source: &DiscoveredSource) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        source.class.as_str(),
        source.physical_path,
        source.logical_path,
        source.module,
        source.input
    )
}

fn canonical_path(field: &'static str, value: String) -> Result<String, DiscoveryError> {
    if value == "." || value.is_empty() {
        return if field == "repository_root" && value.is_empty() {
            Ok(String::new())
        } else {
            Err(DiscoveryError::InvalidField {
                field,
                message: "path must be non-empty and relative".into(),
            })
        };
    }
    if value.starts_with('/')
        || value.contains(['\\', '\n', '\r'])
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(DiscoveryError::InvalidField {
            field,
            message: "path must be relative, slash-separated and canonical".into(),
        });
    }
    Ok(value)
}

fn path_within(root: &str, path: &str) -> bool {
    if root.is_empty() {
        return true;
    }
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> DiscoveryConfig {
        DiscoveryConfig::new(
            "",
            vec![
                DiscoveryRoot::new(TestSourceClass::Production, "src", "src").unwrap(),
                DiscoveryRoot::new(TestSourceClass::UnitTest, "src", "src").unwrap(),
                DiscoveryRoot::new(TestSourceClass::IntegrationTest, "tests", "tests").unwrap(),
            ],
        )
        .unwrap()
    }

    fn entries() -> Vec<DiscoveryEntry> {
        vec![
            DiscoveryEntry::new("src/math_test.to", "src/math_test.to", "math"),
            DiscoveryEntry::new("tests/math_test.to", "tests/math_test.to", "math"),
            DiscoveryEntry::new("src/math.to", "src/math.to", "math"),
        ]
    }

    #[test]
    fn conventional_classification_is_case_sensitive_and_tests_precede_unit_suffix() {
        let sources = discover(&config(), entries()).unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].class(), TestSourceClass::Production);
        assert_eq!(sources[1].class(), TestSourceClass::UnitTest);
        assert_eq!(sources[2].class(), TestSourceClass::IntegrationTest);
        assert_eq!(
            sources[2].input(),
            "source:integration-test:tests/math_test.to"
        );
    }

    #[test]
    fn discovery_order_is_bytewise_and_does_not_depend_on_input_order() {
        let mut reversed = entries();
        reversed.reverse();
        assert_eq!(
            discover(&config(), entries()).unwrap(),
            discover(&config(), reversed).unwrap()
        );
    }

    #[test]
    fn rejects_unclassified_files_and_root_escapes() {
        let error = discover(
            &config(),
            vec![DiscoveryEntry::new(
                "docs/readme.txt",
                "docs/readme.txt",
                "docs",
            )],
        )
        .unwrap_err();
        assert!(matches!(error, DiscoveryError::Unclassified { .. }));
        let error = discover(
            &config(),
            vec![DiscoveryEntry::new(
                "src/../outside.to",
                "src/outside.to",
                "outside",
            )],
        )
        .unwrap_err();
        assert!(matches!(error, DiscoveryError::InvalidField { .. }));
    }

    #[test]
    fn rejects_symlink_escapes_non_regular_files_and_duplicates() {
        let error = discover(
            &config(),
            vec![
                DiscoveryEntry::new("src/math.to", "src/math.to", "math")
                    .with_file_state(true, true),
            ],
        )
        .unwrap_err();
        assert!(matches!(error, DiscoveryError::SymlinkEscape { .. }));
        let error = discover(
            &config(),
            vec![
                DiscoveryEntry::new("src/math.to", "src/math.to", "math")
                    .with_file_state(false, false),
            ],
        )
        .unwrap_err();
        assert!(matches!(error, DiscoveryError::NotRegularFile { .. }));
        let duplicate = DiscoveryEntry::new("src/math.to", "src/math.to", "math");
        let error = discover(&config(), vec![duplicate.clone(), duplicate]).unwrap_err();
        assert!(matches!(
            error,
            DiscoveryError::Duplicate {
                kind: "physical source",
                ..
            }
        ));
    }

    #[test]
    fn reconciliation_is_exact_and_reports_missing_or_additional_sources() {
        let discovered = discover(&config(), entries()).unwrap();
        let expected = discovered
            .iter()
            .map(|source| ExpectedSource {
                class: source.class(),
                physical_path: source.physical_path().into(),
                logical_path: source.logical_path().into(),
                module: source.module().into(),
                input: source.input().into(),
            })
            .collect::<Vec<_>>();
        reconcile_expected(&expected, &discovered).unwrap();
        let mut changed = expected.clone();
        changed.pop();
        let error = reconcile_expected(&changed, &discovered).unwrap_err();
        assert!(matches!(error, DiscoveryError::PlanDrift { .. }));
    }

    #[test]
    fn canonical_paths_reject_dot_segments_backslashes_and_absolute_values() {
        for value in [
            ".",
            "src/./main.to",
            "src/../main.to",
            "src\\main.to",
            "/src/main.to",
        ] {
            assert!(DiscoveryRoot::new(TestSourceClass::Production, value, "src").is_err());
        }
        assert!(
            DiscoveryConfig::new(
                ".",
                vec![DiscoveryRoot::new(TestSourceClass::Production, "src", "src").unwrap()]
            )
            .is_err()
        );
    }

    #[test]
    fn canonical_root_order_is_independent_of_constructor_order() {
        let ordered = config();
        let reversed = DiscoveryConfig::new(
            "",
            vec![
                DiscoveryRoot::new(TestSourceClass::IntegrationTest, "tests", "tests").unwrap(),
                DiscoveryRoot::new(TestSourceClass::UnitTest, "src", "src").unwrap(),
                DiscoveryRoot::new(TestSourceClass::Production, "src", "src").unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(ordered.roots(), reversed.roots());
        assert_eq!(
            discover(&ordered, entries()).unwrap(),
            discover(&reversed, entries()).unwrap()
        );
    }

    #[test]
    fn rejects_invalid_module_and_duplicate_reconciliation_records() {
        let error = discover(
            &config(),
            vec![DiscoveryEntry::new(
                "src/math.to",
                "src/math.to",
                "not-a-module",
            )],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DiscoveryError::InvalidField {
                field: "entry.module",
                ..
            }
        ));

        let discovered = discover(&config(), entries()).unwrap();
        let expected = discovered
            .iter()
            .map(|source| ExpectedSource {
                class: source.class(),
                physical_path: source.physical_path().into(),
                logical_path: source.logical_path().into(),
                module: source.module().into(),
                input: source.input().into(),
            })
            .collect::<Vec<_>>();
        let mut duplicate = expected.clone();
        duplicate.push(expected[0].clone());
        let error = reconcile_expected(&duplicate, &discovered).unwrap_err();
        assert!(matches!(
            error,
            DiscoveryError::Duplicate {
                kind: "expected source",
                ..
            }
        ));
        let mut duplicate_discovered = discovered.clone();
        duplicate_discovered.push(discovered[0].clone());
        let error = reconcile_expected(&expected, &duplicate_discovered).unwrap_err();
        assert!(matches!(
            error,
            DiscoveryError::Duplicate {
                kind: "discovered source",
                ..
            }
        ));
    }

    #[test]
    fn discovery_accessors_plan_bridge_and_error_messages_are_exercised() {
        let root = DiscoveryRoot::new(TestSourceClass::UnitTest, "src", "logical").unwrap();
        assert_eq!(root.class(), TestSourceClass::UnitTest);
        assert_eq!(root.physical_path(), "src");
        assert_eq!(root.logical_path(), "logical");
        let entry = DiscoveryEntry::new("src/main_test.to", "src/main_test.to", "main");
        assert_eq!(entry, entry.clone().with_file_state(true, false));

        let config = DiscoveryConfig::new("", vec![root]).unwrap();
        assert_eq!(config.repository_root(), "");
        assert_eq!(config.roots().len(), 1);
        assert!(DiscoveryConfig::new("", Vec::new()).is_err());
        assert!(
            DiscoveryConfig::new(
                "",
                vec![
                    DiscoveryRoot::new(TestSourceClass::UnitTest, "src", "logical").unwrap(),
                    DiscoveryRoot::new(TestSourceClass::UnitTest, "src", "logical").unwrap(),
                ],
            )
            .is_err()
        );

        let source = discover(
            &config,
            vec![DiscoveryEntry::new(
                "src/main_test.to",
                "logical/main_test.to",
                "main",
            )],
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(source.class(), TestSourceClass::UnitTest);
        assert_eq!(source.physical_path(), "src/main_test.to");
        assert_eq!(source.logical_path(), "logical/main_test.to");
        assert_eq!(source.module(), "main");
        assert_eq!(source.input(), "source:unit-test:src/main_test.to");
        assert!(
            source_key(&ExpectedSource {
                class: source.class(),
                physical_path: source.physical_path().into(),
                logical_path: source.logical_path().into(),
                module: source.module().into(),
                input: source.input().into(),
            })
            .contains("unit-test")
        );
        assert!(discovered_key(&source).contains("unit-test"));

        for error in [
            DiscoveryError::InvalidField {
                field: "x",
                message: "bad".into(),
            },
            DiscoveryError::Unclassified {
                physical_path: "x".into(),
            },
            DiscoveryError::SymlinkEscape {
                physical_path: "x".into(),
            },
            DiscoveryError::NotRegularFile {
                physical_path: "x".into(),
            },
            DiscoveryError::Duplicate {
                kind: "x",
                value: "v".into(),
            },
            DiscoveryError::PlanDrift {
                missing: vec!["m".into()],
                additional: vec!["a".into()],
            },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
