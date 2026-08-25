//! Lossless-enough operational JUnit projection of the canonical test report.
//!
//! JSON remains the normative representation.  This module only projects the
//! aggregate result into XML 1.0 while retaining every report field in ordered
//! `tondo.*` properties.  It has no XML dependency so the output grammar and
//! escaping remain closed and deterministic.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::test_report::{ReportError, TEST_REPORT_FORMAT, TestReport};
use crate::test_result::{AggregateStatus, AttemptStatus, FailureRecord, TestAttempt, TestNode};

pub const JUNIT_FORMAT: &str = "tondo-junit-report-0.1/4";

/// Attempt duration input in nanoseconds.  Missing entries are zero; entries
/// are indexed by the one-based attempt index in the report.
pub type AttemptTimings = BTreeMap<String, Vec<u64>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JUnitReport {
    bytes: Vec<u8>,
}

impl JUnitReport {
    pub fn from_report(report: &TestReport) -> Result<Self, JUnitError> {
        Self::from_report_with_timings(report, &AttemptTimings::new())
    }

    pub fn from_report_with_timings(
        report: &TestReport,
        timings: &AttemptTimings,
    ) -> Result<Self, JUnitError> {
        validate_timings(report, timings)?;
        let containers = build_containers(report, timings)?;
        let mut writer = XmlWriter::default();
        let totals = containers
            .iter()
            .try_fold(Counts::default(), |mut total, container| {
                total.add(container.counts).map(|()| total)
            })?;
        writer.raw("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        writer.open_attrs(
            "testsuites",
            &[
                ("tests", totals.tests.to_string()),
                ("failures", totals.failures.to_string()),
                ("errors", totals.errors.to_string()),
                ("skipped", totals.skipped.to_string()),
                ("time", seconds(totals.time_ns)?),
            ],
        );
        for (index, container) in containers.iter().enumerate() {
            write_container(&mut writer, report, container, timings, index == 0)?;
        }
        writer.close("testsuites");
        writer.raw("\n");
        Ok(Self {
            bytes: writer.into_bytes(),
        })
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JUnitError {
    Report(Box<ReportError>),
    InvalidTiming(String),
    DurationOverflow,
    XmlScalar(u32),
}

impl fmt::Display for JUnitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Report(error) => error.fmt(formatter),
            Self::InvalidTiming(message) => write!(formatter, "invalid JUnit timing: {message}"),
            Self::DurationOverflow => formatter.write_str("JUnit duration exceeds u64 nanoseconds"),
            Self::XmlScalar(value) => write!(formatter, "invalid XML scalar U+{value:X}"),
        }
    }
}

impl Error for JUnitError {}

impl From<ReportError> for JUnitError {
    fn from(error: ReportError) -> Self {
        Self::Report(Box::new(error))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    tests: u32,
    failures: u32,
    errors: u32,
    skipped: u32,
    time_ns: u64,
}

impl Counts {
    fn add(&mut self, other: Self) -> Result<(), JUnitError> {
        self.tests += other.tests;
        self.failures += other.failures;
        self.errors += other.errors;
        self.skipped += other.skipped;
        self.time_ns = self
            .time_ns
            .checked_add(other.time_ns)
            .ok_or(JUnitError::DurationOverflow)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Container {
    id: String,
    cases: Vec<Case>,
    node: Option<TestNode>,
    counts: Counts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Case {
    name: String,
    classname: String,
    time_ns: u64,
    properties: Vec<Property>,
    outcome: Outcome,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failure {
        kind: String,
        message: String,
        body: String,
    },
    Error {
        kind: String,
        message: String,
        body: String,
    },
    Skipped {
        message: String,
    },
}

impl Outcome {
    fn counts(&self) -> Counts {
        match self {
            Self::Passed => Counts {
                tests: 1,
                ..Counts::default()
            },
            Self::Failure { .. } => Counts {
                tests: 1,
                failures: 1,
                ..Counts::default()
            },
            Self::Error { .. } => Counts {
                tests: 1,
                errors: 1,
                ..Counts::default()
            },
            Self::Skipped { .. } => Counts {
                tests: 1,
                skipped: 1,
                ..Counts::default()
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Property {
    name: String,
    value: String,
}

fn build_containers(
    report: &TestReport,
    timings: &AttemptTimings,
) -> Result<Vec<Container>, JUnitError> {
    let mut containers = Vec::new();
    let mut suites = report.suites().to_vec();
    suites.sort_by(|left, right| left.id.cmp(&right.id));
    let mut tests = report.tests().to_vec();
    tests.sort_by(|left, right| left.id.cmp(&right.id));

    for suite in suites {
        let mut cases = tests
            .iter()
            .filter(|test| test.parent.as_deref() == Some(suite.id.as_str()))
            .map(|test| case_for_node(report, test, &suite.id, timings))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(synthetic) = suite_lifecycle_case(&suite, timings)? {
            cases.push(synthetic);
        }
        if suite.status == AggregateStatus::FlakyPass {
            cases.push(flaky_suite_case(report, &suite, timings)?);
        }
        sort_cases(&mut cases);
        containers.push(Container {
            id: suite.id.clone(),
            counts: counts_for(&cases)?,
            cases,
            node: Some(suite),
        });
    }

    let mut top_level: BTreeMap<String, Vec<TestNode>> = BTreeMap::new();
    for test in tests.into_iter().filter(|test| test.parent.is_none()) {
        let key = module_container_id(&test)?;
        top_level.entry(key).or_default().push(test);
    }
    for (id, nodes) in top_level {
        let mut cases = nodes
            .iter()
            .map(|test| case_for_node(report, test, &id, timings))
            .collect::<Result<Vec<_>, _>>()?;
        sort_cases(&mut cases);
        containers.push(Container {
            id,
            counts: counts_for(&cases)?,
            cases,
            node: None,
        });
    }

    containers.sort_by(|left, right| left.id.cmp(&right.id));
    if containers.is_empty() {
        containers.push(Container {
            id: "@tondo-plan".into(),
            cases: Vec::new(),
            node: None,
            counts: Counts::default(),
        });
    }
    Ok(containers)
}

fn case_for_node(
    report: &TestReport,
    node: &TestNode,
    classname: &str,
    timings: &AttemptTimings,
) -> Result<Case, JUnitError> {
    let decisive = decisive_attempt(node)?;
    let mut outcome = outcome_for_node(report, node, decisive)?;
    if report.metadata().repeat.count > 1
        && node
            .attempts
            .iter()
            .any(|attempt| attempt.status != AttemptStatus::Passed)
        && matches!(outcome, Outcome::Passed | Outcome::Skipped { .. })
    {
        outcome = Outcome::Failure {
            kind: "tondo.repeat-instability".into(),
            message: "repeat observed a non-passing attempt".into(),
            body: compact_json(&node.attempts)?,
        };
    }
    Ok(Case {
        name: node.name.clone(),
        classname: classname.into(),
        time_ns: duration_for(node, timings)?,
        properties: node_properties(node, decisive)?,
        outcome,
        stdout: decisive.stdout.clone(),
        stderr: decisive.stderr.clone(),
    })
}

fn suite_lifecycle_case(
    suite: &TestNode,
    timings: &AttemptTimings,
) -> Result<Option<Case>, JUnitError> {
    let decisive = decisive_attempt(suite)?;
    let Some(phase) = decisive.phase else {
        return Ok(None);
    };
    if decisive.status == AttemptStatus::Passed {
        return Ok(None);
    }
    let name = match phase {
        crate::test_result::AttemptPhase::Setup => "@setup",
        crate::test_result::AttemptPhase::Teardown => "@teardown",
    };
    let id = format!("{}::{name}", suite.id);
    let mut properties = node_properties(suite, decisive)?;
    properties.push(Property {
        name: "tondo.synthetic".into(),
        value: "suite-lifecycle".into(),
    });
    Ok(Some(Case {
        name: name.into(),
        classname: suite.id.clone(),
        time_ns: duration_for(suite, timings)?,
        properties: replace_property(properties, "tondo.id", &id),
        outcome: outcome_for_attempt(decisive)?,
        stdout: decisive.stdout.clone(),
        stderr: decisive.stderr.clone(),
    }))
}

fn flaky_suite_case(
    report: &TestReport,
    suite: &TestNode,
    timings: &AttemptTimings,
) -> Result<Case, JUnitError> {
    let decisive = decisive_attempt(suite)?;
    let id = format!("{}::@flaky", suite.id);
    let mut properties = node_properties(suite, decisive)?;
    properties.push(Property {
        name: "tondo.synthetic".into(),
        value: "flaky-policy".into(),
    });
    let outcome = if report.metadata().policy.allow_flaky {
        Outcome::Passed
    } else {
        Outcome::Failure {
            kind: "tondo.flaky-pass".into(),
            message: "passed after a prior non-pass".into(),
            body: compact_json(&suite.attempts)?,
        }
    };
    Ok(Case {
        name: "@flaky".into(),
        classname: suite.id.clone(),
        time_ns: duration_for(suite, timings)?,
        properties: replace_property(properties, "tondo.id", &id),
        outcome,
        stdout: decisive.stdout.clone(),
        stderr: decisive.stderr.clone(),
    })
}

fn outcome_for_node(
    report: &TestReport,
    node: &TestNode,
    attempt: &TestAttempt,
) -> Result<Outcome, JUnitError> {
    if node.status == AggregateStatus::FlakyPass {
        return Ok(if report.metadata().policy.allow_flaky {
            Outcome::Passed
        } else {
            Outcome::Failure {
                kind: "tondo.flaky-pass".into(),
                message: "passed after a prior non-pass".into(),
                body: compact_json(&node.attempts)?,
            }
        });
    }
    outcome_for_attempt(attempt)
}

fn outcome_for_attempt(attempt: &TestAttempt) -> Result<Outcome, JUnitError> {
    match attempt.status {
        AttemptStatus::Passed => Ok(Outcome::Passed),
        AttemptStatus::Skipped | AttemptStatus::BlockedSetup | AttemptStatus::BlockedSkip => {
            let message = attempt
                .skip
                .as_ref()
                .map(|skip| skip.reason.clone())
                .or_else(|| {
                    attempt
                        .blocked_by
                        .as_ref()
                        .map(|blocked| format!("blocked by {}", blocked.id))
                })
                .unwrap_or_else(|| "skipped".into());
            Ok(Outcome::Skipped { message })
        }
        AttemptStatus::FailedError | AttemptStatus::FailedPanic => {
            let failure = attempt.failure.as_ref().ok_or_else(|| {
                JUnitError::InvalidTiming("failure attempt has no failure payload".into())
            })?;
            Ok(Outcome::Failure {
                kind: failure_type(failure),
                message: failure.message.clone(),
                body: compact_json(failure)?,
            })
        }
        AttemptStatus::ResourceLimit | AttemptStatus::Timeout | AttemptStatus::Infrastructure => {
            let failure = attempt.failure.as_ref().ok_or_else(|| {
                JUnitError::InvalidTiming("error attempt has no failure payload".into())
            })?;
            Ok(Outcome::Error {
                kind: failure_type(failure),
                message: failure.message.clone(),
                body: compact_json(failure)?,
            })
        }
    }
}

fn failure_type(failure: &FailureRecord) -> String {
    failure.code.clone().unwrap_or_else(|| failure.kind.clone())
}

fn decisive_attempt(node: &TestNode) -> Result<&TestAttempt, JUnitError> {
    node.attempts
        .get(node.decisive_attempt.saturating_sub(1) as usize)
        .ok_or_else(|| {
            JUnitError::InvalidTiming("decisive attempt is outside the node history".into())
        })
}

fn duration_for(node: &TestNode, timings: &AttemptTimings) -> Result<u64, JUnitError> {
    let values = timings.get(&node.id);
    let mut total = 0_u64;
    for attempt in &node.attempts {
        let duration = values
            .and_then(|entries| entries.get(attempt.index.saturating_sub(1) as usize))
            .copied()
            .unwrap_or(0);
        total = total
            .checked_add(duration)
            .ok_or(JUnitError::DurationOverflow)?;
    }
    Ok(total)
}

fn validate_timings(report: &TestReport, timings: &AttemptTimings) -> Result<(), JUnitError> {
    let known = report
        .suites()
        .iter()
        .chain(report.tests())
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    for (id, values) in timings {
        if !known.contains(id.as_str()) {
            return Err(JUnitError::InvalidTiming(format!("unknown node `{id}`")));
        }
        let node = report
            .suites()
            .iter()
            .chain(report.tests())
            .find(|node| node.id == *id)
            .expect("known timing ID must have a node");
        if values.len() > node.attempts.len() {
            return Err(JUnitError::InvalidTiming(format!(
                "timing vector for `{id}` is longer than attempts"
            )));
        }
    }
    Ok(())
}

fn node_properties(node: &TestNode, decisive: &TestAttempt) -> Result<Vec<Property>, JUnitError> {
    let source_kind = source_class(&node.id)?;
    let mut properties = Vec::new();
    push_json(&mut properties, "tondo.id", &node.id)?;
    push_json(&mut properties, "tondo.parent", &node.parent)?;
    push_json(&mut properties, "tondo.package", &node.package)?;
    push_json(&mut properties, "tondo.kind", &source_kind)?;
    push_json(&mut properties, "tondo.module", &node.module)?;
    push_json(&mut properties, "tondo.path", &node.path)?;
    push_json(&mut properties, "tondo.name", &node.name)?;
    push_json(&mut properties, "tondo.status", &node.status)?;
    push_json(
        &mut properties,
        "tondo.decisive_attempt",
        &node.decisive_attempt,
    )?;
    push_json(&mut properties, "tondo.attempts", &node.attempts)?;
    push_json(&mut properties, "tondo.artifacts", &decisive.artifacts)?;
    push_json(&mut properties, "tondo.diagnostics", &decisive.diagnostics)?;
    push_json(&mut properties, "tondo.snapshots", &decisive.snapshots)?;
    push_json(
        &mut properties,
        "tondo.virtual_time",
        &decisive.virtual_time,
    )?;
    push_json(&mut properties, "tondo.source", &node.source)?;
    push_json(&mut properties, "tondo.owners", &node.owners)?;
    Ok(properties)
}

fn execution_properties(report: &TestReport) -> Result<Vec<Property>, JUnitError> {
    let metadata = report.metadata();
    let mut properties = Vec::new();
    push_json(&mut properties, "tondo.format", &JUNIT_FORMAT)?;
    push_json(&mut properties, "tondo.json_format", &TEST_REPORT_FORMAT)?;
    push_json(&mut properties, "tondo.edition", &metadata.edition)?;
    push_json(&mut properties, "tondo.target", &metadata.target)?;
    push_json(&mut properties, "tondo.compiled", &metadata.compiled)?;
    push_json(&mut properties, "tondo.selection", &metadata.selection)?;
    push_json(&mut properties, "tondo.ownership", &metadata.ownership)?;
    push_json(&mut properties, "tondo.inputs", &metadata.inputs)?;
    push_json(&mut properties, "tondo.shard", &metadata.shard)?;
    push_json(&mut properties, "tondo.order", &metadata.order)?;
    push_json(&mut properties, "tondo.seed", &metadata.order.seed)?;
    push_json(
        &mut properties,
        "tondo.execution_plan",
        &report.execution_plan(),
    )?;
    push_json(&mut properties, "tondo.retry", &metadata.retry)?;
    push_json(&mut properties, "tondo.repeat", &metadata.repeat)?;
    push_json(
        &mut properties,
        "tondo.artifact_store",
        &metadata.artifact_store,
    )?;
    push_json(
        &mut properties,
        "tondo.snapshot_policy",
        &metadata.snapshot_policy,
    )?;
    push_json(&mut properties, "tondo.policy", &metadata.policy)?;
    push_json(&mut properties, "tondo.limits", &metadata.limits)?;
    push_json(&mut properties, "tondo.summary", report.summary())?;
    Ok(properties)
}

fn push_json<T: Serialize>(
    properties: &mut Vec<Property>,
    name: &str,
    value: &T,
) -> Result<(), JUnitError> {
    let value = serde_json::to_value(value).map_err(|error| {
        JUnitError::Report(Box::new(ReportError::Serialization(error.to_string())))
    })?;
    properties.push(Property {
        name: name.into(),
        value: property_value(&value)?,
    });
    Ok(())
}

fn property_value(value: &Value) -> Result<String, JUnitError> {
    if let Value::String(value) = value {
        Ok(value.clone())
    } else {
        serde_json::to_string(value).map_err(|error| {
            JUnitError::Report(Box::new(ReportError::Serialization(error.to_string())))
        })
    }
}

fn compact_json<T: Serialize>(value: &T) -> Result<String, JUnitError> {
    serde_json::to_string(value).map_err(|error| {
        JUnitError::Report(Box::new(ReportError::Serialization(error.to_string())))
    })
}

fn replace_property(mut properties: Vec<Property>, name: &str, value: &str) -> Vec<Property> {
    if let Some(property) = properties.iter_mut().find(|property| property.name == name) {
        property.value = value.into();
    }
    properties
}

fn counts_for(cases: &[Case]) -> Result<Counts, JUnitError> {
    let mut counts = Counts::default();
    for case in cases {
        counts.add(case.outcome.counts())?;
        counts.time_ns = counts
            .time_ns
            .checked_add(case.time_ns)
            .ok_or(JUnitError::DurationOverflow)?;
    }
    Ok(counts)
}

fn sort_cases(cases: &mut [Case]) {
    cases.sort_by(|left, right| {
        synthetic_rank(&left.name)
            .cmp(&synthetic_rank(&right.name))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.classname.cmp(&right.classname))
    });
}

fn synthetic_rank(name: &str) -> u8 {
    match name {
        "@setup" => 0,
        "@flaky" => 1,
        "@teardown" => 3,
        _ => 2,
    }
}

fn module_container_id(node: &TestNode) -> Result<String, JUnitError> {
    Ok(format!(
        "{}::{}::{}",
        node.package,
        source_class(&node.id)?,
        node.module
    ))
}

fn source_class(id: &str) -> Result<&'static str, JUnitError> {
    match id.split("::").nth(1).unwrap_or("unit") {
        "unit" => Ok("unit"),
        "integration" => Ok("integration"),
        _ => Err(JUnitError::InvalidTiming(format!(
            "node ID `{id}` has no valid source class"
        ))),
    }
}

fn write_container(
    writer: &mut XmlWriter,
    report: &TestReport,
    container: &Container,
    timings: &AttemptTimings,
    metadata_carrier: bool,
) -> Result<(), JUnitError> {
    let mut properties = if metadata_carrier {
        execution_properties(report)?
    } else {
        Vec::new()
    };
    if let Some(node) = &container.node {
        if !metadata_carrier {
            properties = node_properties(node, decisive_attempt(node)?)?;
        } else {
            properties.extend(node_properties(node, decisive_attempt(node)?)?);
        }
    }
    if container.id == "@tondo-plan" {
        properties.push(Property {
            name: "tondo.synthetic".into(),
            value: "empty-plan".into(),
        });
    }
    writer.open_attrs(
        "testsuite",
        &[
            ("name", container.id.clone()),
            ("tests", container.counts.tests.to_string()),
            ("failures", container.counts.failures.to_string()),
            ("errors", container.counts.errors.to_string()),
            ("skipped", container.counts.skipped.to_string()),
            ("time", seconds(container.counts.time_ns)?),
        ],
    );
    if properties.is_empty() && container.cases.is_empty() {
        writer.close_empty();
        return Ok(());
    }
    if !properties.is_empty() {
        writer.open("properties");
        for property in properties {
            writer.empty_attrs(
                "property",
                &[("name", property.name), ("value", property.value)],
            );
        }
        writer.close("properties");
    }
    for case in &container.cases {
        write_case(writer, case)?;
    }
    writer.close("testsuite");
    let _ = timings;
    Ok(())
}

fn write_case(writer: &mut XmlWriter, case: &Case) -> Result<(), JUnitError> {
    let attrs = vec![
        ("name", case.name.clone()),
        ("classname", case.classname.clone()),
        ("time", seconds(case.time_ns)?),
    ];
    if case.properties.is_empty()
        && matches!(&case.outcome, Outcome::Passed)
        && case.stdout.is_empty()
        && case.stderr.is_empty()
    {
        writer.empty_attrs("testcase", &attrs);
        return Ok(());
    }
    writer.open_attrs("testcase", &attrs);
    if !case.properties.is_empty() {
        writer.open("properties");
        for property in &case.properties {
            writer.empty_attrs(
                "property",
                &[
                    ("name", property.name.clone()),
                    ("value", property.value.clone()),
                ],
            );
        }
        writer.close("properties");
    }
    match &case.outcome {
        Outcome::Passed => {}
        Outcome::Skipped { message } => {
            writer.empty_attrs("skipped", &[("message", message.clone())])
        }
        Outcome::Failure {
            kind,
            message,
            body,
        } => {
            writer.element_attrs_text(
                "failure",
                &[("type", kind.clone()), ("message", message.clone())],
                body,
            );
        }
        Outcome::Error {
            kind,
            message,
            body,
        } => {
            writer.element_attrs_text(
                "error",
                &[("type", kind.clone()), ("message", message.clone())],
                body,
            );
        }
    }
    if !case.stdout.is_empty() {
        writer.element_text("system-out", &case.stdout);
    }
    if !case.stderr.is_empty() {
        writer.element_text("system-err", &case.stderr);
    }
    writer.close("testcase");
    Ok(())
}

fn seconds(nanoseconds: u64) -> Result<String, JUnitError> {
    let seconds = nanoseconds / 1_000_000_000;
    let remainder = nanoseconds % 1_000_000_000;
    if remainder == 0 {
        return Ok(seconds.to_string());
    }
    let fraction = format!("{remainder:09}");
    Ok(format!("{seconds}.{}", fraction.trim_end_matches('0')))
}

#[derive(Default)]
struct XmlWriter {
    output: String,
    open: Vec<String>,
}

impl XmlWriter {
    fn raw(&mut self, value: &str) {
        self.output.push_str(value);
    }

    fn open(&mut self, name: &str) {
        self.output.push('<');
        self.output.push_str(name);
        self.output.push('>');
        self.open.push(name.into());
    }

    fn open_attrs(&mut self, name: &str, attributes: &[(impl AsRef<str>, String)]) {
        self.output.push('<');
        self.output.push_str(name);
        for (key, value) in attributes {
            self.output.push(' ');
            self.output.push_str(key.as_ref());
            self.output.push_str("=\"");
            self.output.push_str(&xml_scalar(value));
            self.output.push('"');
        }
        self.output.push('>');
        self.open.push(name.into());
    }

    fn empty_attrs(&mut self, name: &str, attributes: &[(impl AsRef<str>, String)]) {
        self.output.push('<');
        self.output.push_str(name);
        for (key, value) in attributes {
            self.output.push(' ');
            self.output.push_str(key.as_ref());
            self.output.push_str("=\"");
            self.output.push_str(&xml_scalar(value));
            self.output.push('"');
        }
        self.output.push_str("/>");
    }

    fn element_text(&mut self, name: &str, text: &str) {
        self.output.push('<');
        self.output.push_str(name);
        self.output.push('>');
        self.output.push_str(&xml_scalar(text));
        self.output.push_str("</");
        self.output.push_str(name);
        self.output.push('>');
    }

    fn element_attrs_text(&mut self, name: &str, attrs: &[(impl AsRef<str>, String)], text: &str) {
        self.open_attrs(name, attrs);
        self.output.push_str(&xml_scalar(text));
        self.close(name);
    }

    fn close(&mut self, name: &str) {
        debug_assert_eq!(self.open.pop().as_deref(), Some(name));
        self.output.push_str("</");
        self.output.push_str(name);
        self.output.push('>');
    }

    fn close_empty(&mut self) {
        if self.output.ends_with('>') {
            self.output.pop();
            self.output.push_str("/>");
        }
        self.open.pop();
    }

    fn into_bytes(self) -> Vec<u8> {
        self.output.into_bytes()
    }
}

fn xml_scalar(value: &str) -> String {
    let mut output = String::new();
    for scalar in value.chars() {
        let code = scalar as u32;
        if !is_xml_scalar(code) {
            output.push_str(&format!("\\u{{{code:X}}}"));
            continue;
        }
        match scalar {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '\"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(scalar),
        }
    }
    output
}

fn is_xml_scalar(value: u32) -> bool {
    matches!(value, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_report::{ReportMetadata, SnapshotMode, TestReport};
    use crate::test_result::{AttemptPhase, AttemptStatus, ResultNodeKind, TestAttempt, TestNode};

    fn hash(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn metadata() -> ReportMetadata {
        let mut metadata = ReportMetadata::default();
        metadata.inputs.public_sha256 = hash('a');
        metadata.snapshot_policy.before_sha256 = hash('b');
        metadata.snapshot_policy.after_sha256 = hash('b');
        metadata.limits.resource_profile_sha256 = hash('c');
        metadata
    }

    fn node(id: &str, kind: ResultNodeKind) -> TestNode {
        let parent = (kind == ResultNodeKind::Test).then(|| "application::unit::math".into());
        TestNode::new(
            id,
            parent,
            "application",
            kind,
            "math",
            id.rsplit("::").next().unwrap_or(id),
            vec![TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed)],
        )
    }

    fn report() -> TestReport {
        TestReport::assemble(
            metadata(),
            vec!["application::unit::math::adds".into()],
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![node("application::unit::math::adds", ResultNodeKind::Test)],
        )
        .unwrap()
    }

    #[test]
    fn emits_declaration_properties_and_aggregated_time() {
        let mut timings = AttemptTimings::new();
        timings.insert("application::unit::math::adds".into(), vec![1_500_000_000]);
        let junit = JUnitReport::from_report_with_timings(&report(), &timings)
            .unwrap()
            .into_bytes();
        let xml = String::from_utf8(junit).unwrap();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites"));
        assert!(xml.contains("tondo.format"));
        assert!(xml.contains("time=\"1.5\""));
        assert!(xml.contains("tondo.json_format"));
        assert!(xml.contains("classname=\"application::unit::math\""));
        assert!(xml.ends_with("</testsuites>\n"));
    }

    #[test]
    fn projects_failures_skips_flaky_and_repeat_instability_without_duplicate_leaf_cases() {
        let mut metadata = metadata();
        metadata.repeat.count = 2;
        metadata.policy.allow_flaky = true;
        let mut fail = node("application::unit::math::fails", ResultNodeKind::Test);
        fail.attempts[0].status = AttemptStatus::FailedPanic;
        fail.attempts[0].failure = Some(crate::test_result::FailureRecord {
            kind: "panic".into(),
            code: Some("P0007".into()),
            message: "bad <value>".into(),
            source: None,
        });
        let mut flaky = node("application::unit::math::flaky", ResultNodeKind::Test);
        flaky.attempts = vec![
            {
                let mut attempt = TestAttempt::new(1, 1, 0, None, AttemptStatus::Skipped);
                attempt.skip = Some(crate::test_result::SkipRecord {
                    reason: "later".into(),
                    source: None,
                });
                attempt
            },
            TestAttempt::new(2, 1, 1, Some(1), AttemptStatus::Passed),
        ];
        let report = TestReport::assemble(
            metadata,
            vec![fail.id.clone(), flaky.id.clone()],
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![fail, flaky],
        )
        .unwrap();
        let xml =
            String::from_utf8(JUnitReport::from_report(&report).unwrap().into_bytes()).unwrap();
        assert_eq!(xml.matches("<testcase ").count(), 2);
        assert!(xml.contains("tondo.repeat-instability"));
        assert!(xml.contains("P0007"));
        assert!(xml.contains("&lt;value&gt;"));
    }

    #[test]
    fn suite_lifecycle_and_flaky_policy_create_synthetic_cases() {
        let mut metadata = metadata();
        metadata.policy.allow_flaky = false;
        let mut suite = node("application::unit::math", ResultNodeKind::Suite);
        suite.attempts[0].status = AttemptStatus::FailedPanic;
        suite.attempts[0].phase = Some(AttemptPhase::Setup);
        suite.attempts[0].failure = Some(crate::test_result::FailureRecord {
            kind: "panic".into(),
            code: None,
            message: "setup".into(),
            source: None,
        });
        let mut test = node("application::unit::math::adds", ResultNodeKind::Test);
        test.attempts[0].status = AttemptStatus::BlockedSetup;
        test.attempts[0].blocked_by = Some(crate::test_result::BlockedBy {
            id: suite.id.clone(),
            attempt: 1,
        });
        let report =
            TestReport::assemble(metadata, vec![test.id.clone()], vec![suite], vec![test]).unwrap();
        let xml =
            String::from_utf8(JUnitReport::from_report(&report).unwrap().into_bytes()).unwrap();
        assert!(xml.contains("@setup"));
        assert!(xml.contains("suite-lifecycle"));
        assert!(xml.contains("blocked by application::unit::math"));
    }

    #[test]
    fn flaky_suite_policy_emits_one_explicit_synthetic_case() {
        let mut metadata = metadata();
        metadata.policy.allow_flaky = true;
        let mut suite = node("application::unit::math", ResultNodeKind::Suite);
        let mut failed = TestAttempt::new(1, 1, 0, None, AttemptStatus::FailedPanic);
        failed.failure = Some(crate::test_result::FailureRecord {
            kind: "panic".into(),
            code: Some("P0007".into()),
            message: "first attempt failed".into(),
            source: None,
        });
        suite.attempts = vec![
            failed,
            TestAttempt::new(2, 1, 1, Some(1), AttemptStatus::Passed),
        ];
        let test = node("application::unit::math::adds", ResultNodeKind::Test);
        let report =
            TestReport::assemble(metadata, vec![test.id.clone()], vec![suite], vec![test]).unwrap();
        let xml =
            String::from_utf8(JUnitReport::from_report(&report).unwrap().into_bytes()).unwrap();
        assert!(xml.contains("@flaky"));
        assert!(xml.contains("flaky-policy"));
    }

    #[test]
    fn update_snapshot_policy_and_empty_plan_are_serialized_without_execution_payloads() {
        let mut metadata = metadata();
        metadata.snapshot_policy.mode = SnapshotMode::Update;
        metadata.snapshot_policy.published = Some(false);
        let report = TestReport::assemble(metadata, vec![], vec![], vec![]).unwrap();
        let xml =
            String::from_utf8(JUnitReport::from_report(&report).unwrap().into_bytes()).unwrap();
        assert!(xml.contains("@tondo-plan"));
        assert!(xml.contains("name=\"tondo.synthetic\" value=\"empty-plan\""));
        assert!(xml.contains("tondo.snapshot_policy"));
        assert!(xml.contains("tests=\"0\""));
    }

    #[test]
    fn rejects_unknown_timings_overlong_vectors_and_duration_overflow() {
        let mut timings = AttemptTimings::new();
        timings.insert("missing".into(), vec![1]);
        assert!(JUnitReport::from_report_with_timings(&report(), &timings).is_err());
        timings.clear();
        timings.insert("application::unit::math::adds".into(), vec![1, 2]);
        assert!(JUnitReport::from_report_with_timings(&report(), &timings).is_err());
        let mut adds = node("application::unit::math::adds", ResultNodeKind::Test);
        adds.attempts
            .push(TestAttempt::new(2, 1, 0, None, AttemptStatus::Passed));
        let report = TestReport::assemble(
            metadata(),
            vec![adds.id.clone()],
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![adds],
        )
        .unwrap();
        timings.insert("application::unit::math::adds".into(), vec![u64::MAX, 1]);
        assert!(JUnitReport::from_report_with_timings(&report, &timings).is_err());
    }

    #[test]
    fn xml_scalar_escapes_controls_and_attributes_without_external_entities() {
        assert_eq!(xml_scalar("<&\"'\0"), "&lt;&amp;&quot;&apos;\\u{0}");
        assert!(is_xml_scalar('x' as u32));
        assert!(!is_xml_scalar(0));
        assert_eq!(seconds(0).unwrap(), "0");
        assert_eq!(seconds(1_000_000_001).unwrap(), "1.000000001");
    }

    #[test]
    fn public_junit_views_and_closed_error_shapes_are_total() {
        let report = report();
        let junit = JUnitReport::from_report(&report).unwrap();
        assert!(!std::hint::black_box(junit.canonical_bytes()).is_empty());
        assert!(!std::hint::black_box(junit.clone().into_bytes()).is_empty());
        let mut counts = Counts::default();
        counts.add(Outcome::Passed.counts()).unwrap();
        counts
            .add(
                Outcome::Failure {
                    kind: "panic".into(),
                    message: "failed".into(),
                    body: "{}".into(),
                }
                .counts(),
            )
            .unwrap();
        counts
            .add(
                Outcome::Error {
                    kind: "timeout".into(),
                    message: "slow".into(),
                    body: "{}".into(),
                }
                .counts(),
            )
            .unwrap();
        counts
            .add(
                Outcome::Skipped {
                    message: "skip".into(),
                }
                .counts(),
            )
            .unwrap();
        assert_eq!(
            (counts.tests, counts.failures, counts.errors, counts.skipped),
            (4, 1, 1, 1)
        );
        for error in [
            JUnitError::InvalidTiming("bad".into()),
            JUnitError::DurationOverflow,
            JUnitError::XmlScalar(0),
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
