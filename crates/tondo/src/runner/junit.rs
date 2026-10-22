//! The JUnit XML profile of the report (testing 15.5): for CI, with times;
//! the JSON report stays the complete form.

use super::json::{self, Value};
use super::{Aggregate, Attempt, Options, Status, Tree};
use std::time::Duration;

/// What one `testcase` holds.
struct Case {
    name: String,
    class: String,
    properties: Vec<(String, Value)>,
    /// `failure`, `error` or `skipped`, its type and message, and its body.
    outcome: Option<(&'static str, String, String, String)>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    time: Duration,
}

pub(super) fn report(
    tree: &Tree,
    context: &json::Context,
    aggregates: &[Aggregate],
    report: &Value,
    options: &Options,
) -> String {
    let find = |node: u32| aggregates.iter().find(|a| a.node == node);
    // Containers: each suite, and each module with top-level tests.
    let mut containers: Vec<(String, Option<u32>)> = Vec::new();
    for aggregate in aggregates {
        let node = aggregate.node;
        let container = if tree.suite(node) {
            (tree.id(node).to_string(), Some(node))
        } else if tree.nodes[node as usize].parent == 0 {
            (module_prefix(tree.id(node)), None)
        } else {
            continue;
        };
        if !containers.contains(&container) {
            containers.push(container);
        }
    }
    containers.sort_by(|a, b| a.0.cmp(&b.0));
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let mut suites_xml = Vec::new();
    let mut totals = (0, 0, 0, 0, Duration::ZERO);
    let metadata = run_properties(report);
    if containers.is_empty() {
        let mut properties = metadata.clone();
        properties.push(("tondo.synthetic".into(), Value::Str("empty-plan".into())));
        suites_xml.push(testsuite("@tondo-plan", &properties, &[]).0);
    }
    for (index, (name, suite)) in containers.iter().enumerate() {
        let mut properties = if index == 0 { metadata.clone() } else { Vec::new() };
        let mut cases = Vec::new();
        if let Some(suite) = suite.and_then(find) {
            properties.extend(node_properties(tree, context, suite, aggregates));
            let decisive = &suite.attempts[suite.decisive];
            if suite.status.failed() {
                let phase = if decisive.teardown { "@teardown" } else { "@setup" };
                cases
                    .push((phase, synthetic(tree, suite, phase, "suite-lifecycle", failed_outcome(decisive, options))));
            }
            if suite.flaky {
                let outcome = (!options.allow_flaky).then(|| flaky_outcome(context, suite, aggregates));
                cases.push(("@flaky", synthetic(tree, suite, "@flaky", "flaky-policy", outcome)));
            }
        }
        // The leaves directly inside.
        let mut leaves: Vec<&Aggregate> = aggregates
            .iter()
            .filter(|a| !tree.suite(a.node))
            .filter(|a| match suite {
                Some(suite) => tree.nodes[a.node as usize].parent == *suite,
                None => tree.nodes[a.node as usize].parent == 0 && module_prefix(tree.id(a.node)) == *name,
            })
            .collect();
        leaves.sort_by(|a, b| tree.id(a.node).cmp(tree.id(b.node)));
        let mut ordered: Vec<Case> = Vec::new();
        let (setup, rest): (Vec<_>, Vec<_>) = cases.into_iter().partition(|(n, _)| *n == "@setup");
        let (flaky, teardown): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(n, _)| *n == "@flaky");
        ordered.extend(setup.into_iter().chain(flaky).map(|(_, c)| c));
        ordered.extend(leaves.into_iter().map(|leaf| leaf_case(tree, context, leaf, aggregates, options, name)));
        ordered.extend(teardown.into_iter().map(|(_, c)| c));
        let (text, counts) = testsuite(name, &properties, &ordered);
        totals =
            (totals.0 + counts.0, totals.1 + counts.1, totals.2 + counts.2, totals.3 + counts.3, totals.4 + counts.4);
        suites_xml.push(text);
    }
    xml.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" errors=\"{}\" skipped=\"{}\" time=\"{}\">\n",
        totals.0,
        totals.1,
        totals.2,
        totals.3,
        seconds(totals.4)
    ));
    for suite in suites_xml {
        xml.push_str(&suite);
    }
    xml.push_str("</testsuites>\n");
    xml
}

/// `package::kind::module` of an identity.
fn module_prefix(id: &str) -> String {
    id.split("::").take(3).collect::<Vec<_>>().join("::")
}

/// The run's metadata, from the JSON report.
fn run_properties(report: &Value) -> Vec<(String, Value)> {
    let field = |key: &str| report.get(key).cloned().unwrap_or(Value::Null);
    let seed = report.get("order").and_then(|o| o.get("seed")).cloned().unwrap_or(Value::Null);
    vec![
        ("tondo.format".into(), Value::Str("tondo-junit-report-0.1/1".into())),
        ("tondo.json_format".into(), field("format")),
        ("tondo.edition".into(), field("edition")),
        ("tondo.target".into(), field("target")),
        ("tondo.compiled".into(), field("compiled")),
        ("tondo.selection".into(), field("selection")),
        ("tondo.ownership".into(), field("ownership")),
        ("tondo.shard".into(), field("shard")),
        ("tondo.order".into(), field("order")),
        ("tondo.seed".into(), seed),
        ("tondo.execution_plan".into(), field("execution_plan")),
        ("tondo.retry".into(), field("retry")),
        ("tondo.repeat".into(), field("repeat")),
        ("tondo.artifact_store".into(), field("artifact_store")),
        ("tondo.snapshot_policy".into(), field("snapshot_policy")),
        ("tondo.policy".into(), field("policy")),
        ("tondo.limits".into(), field("limits")),
        ("tondo.summary".into(), field("summary")),
    ]
}

/// The properties of a node, from its JSON descriptor and its decisive
/// attempt.
fn node_properties(
    tree: &Tree,
    context: &json::Context,
    aggregate: &Aggregate,
    all: &[Aggregate],
) -> Vec<(String, Value)> {
    let attempts: Vec<Value> =
        aggregate.attempts.iter().enumerate().map(|(i, a)| context.attempt(i + 1, a, all)).collect();
    let decisive = attempts[aggregate.decisive].clone();
    let encoding = |key: &str| decisive.get(key).and_then(|s| s.get("encoding")).cloned().unwrap_or(Value::Null);
    let id = tree.id(aggregate.node);
    let parts: Vec<&str> = id.split("::").collect();
    let parent = tree.nodes[aggregate.node as usize].parent;
    let status = if aggregate.flaky { "flaky-pass" } else { json::status_name(&aggregate.status) };
    let descriptor = context_descriptor(context, aggregate.node);
    vec![
        ("tondo.id".into(), Value::Str(id.into())),
        ("tondo.parent".into(), if parent == 0 { Value::Null } else { Value::Str(tree.id(parent).into()) }),
        ("tondo.package".into(), Value::Str(parts[0].into())),
        ("tondo.kind".into(), Value::Str(parts[1].into())),
        ("tondo.module".into(), Value::Str(parts[2].into())),
        ("tondo.path".into(), Value::Array(parts[3..].iter().map(|p| Value::Str(p.to_string())).collect())),
        ("tondo.name".into(), Value::Str(parts.last().unwrap_or(&"").to_string())),
        ("tondo.status".into(), Value::Str(status.into())),
        ("tondo.decisive_attempt".into(), Value::Int(aggregate.decisive as i64 + 1)),
        ("tondo.attempts".into(), Value::Array(attempts)),
        ("tondo.stdout.encoding".into(), encoding("stdout")),
        ("tondo.stderr.encoding".into(), encoding("stderr")),
        ("tondo.artifacts".into(), decisive.get("artifacts").cloned().unwrap_or(Value::Null)),
        ("tondo.snapshots".into(), decisive.get("snapshots").cloned().unwrap_or(Value::Null)),
        ("tondo.virtual_time".into(), decisive.get("virtual_time").cloned().unwrap_or(Value::Null)),
        ("tondo.source".into(), descriptor.get("source").cloned().unwrap_or(Value::Null)),
        ("tondo.owners".into(), descriptor.get("owners").cloned().unwrap_or(Value::Null)),
    ]
}

fn context_descriptor(context: &json::Context, node: u32) -> Value {
    Value::Object(context.descriptor_fields(node))
}

fn leaf_case(
    tree: &Tree,
    context: &json::Context,
    leaf: &Aggregate,
    all: &[Aggregate],
    options: &Options,
    class: &str,
) -> Case {
    let decisive = &leaf.attempts[leaf.decisive];
    let mut outcome = match &leaf.status {
        Status::Passed if leaf.flaky && !options.allow_flaky => Some(flaky_outcome(context, leaf, all)),
        Status::Passed => None,
        Status::Skipped(text) => {
            let reason = text.split("\n --> ").next().unwrap_or(text).to_string();
            Some(("skipped", String::new(), reason, String::new()))
        }
        Status::Blocked(cause, _) => {
            Some(("skipped", String::new(), format!("blocked by {}", tree.id(*cause)), String::new()))
        }
        _ => failed_outcome(decisive, options),
    };
    let red = matches!(outcome, Some(("failure" | "error", ..)));
    if options.repeat > 1 && !red && leaf.attempts.iter().any(|a| a.status != Status::Passed) {
        let attempts = attempts_text(context, leaf, all);
        outcome = Some((
            "failure",
            "tondo.repeat-instability".into(),
            "repeat observed a non-passing attempt".into(),
            attempts,
        ));
    }
    Case {
        name: tree.id(leaf.node).rsplit("::").next().unwrap_or_default().to_string(),
        class: class.to_string(),
        properties: node_properties(tree, context, leaf, all),
        outcome,
        stdout: decisive.stdout.clone(),
        stderr: decisive.stderr.clone(),
        time: leaf.attempts.iter().map(|a| a.duration).sum(),
    }
}

fn attempts_text(context: &json::Context, aggregate: &Aggregate, all: &[Aggregate]) -> String {
    let attempts = aggregate.attempts.iter().enumerate().map(|(i, a)| context.attempt(i + 1, a, all)).collect();
    Value::Array(attempts).text_for_xml()
}

fn flaky_outcome(
    context: &json::Context,
    aggregate: &Aggregate,
    all: &[Aggregate],
) -> (&'static str, String, String, String) {
    let attempts = attempts_text(context, aggregate, all);
    ("failure", "tondo.flaky-pass".into(), "passed after a prior non-pass".into(), attempts)
}

/// `failure` for errors and panics, `error` for the rest.
fn failed_outcome(attempt: &Attempt, options: &Options) -> Option<(&'static str, String, String, String)> {
    let failure = json::failure(&attempt.status, options);
    let field = |key: &str| match failure.get(key) {
        Some(Value::Str(text)) => Some(text.clone()),
        _ => None,
    };
    let kind = field("kind")?;
    let element = if matches!(kind.as_str(), "error" | "panic") { "failure" } else { "error" };
    Some((element, field("code").unwrap_or(kind), field("message").unwrap_or_default(), failure.text_for_xml()))
}

fn synthetic(
    tree: &Tree,
    suite: &Aggregate,
    name: &str,
    kind: &str,
    outcome: Option<(&'static str, String, String, String)>,
) -> Case {
    let id = tree.id(suite.node);
    let decisive = &suite.attempts[suite.decisive];
    Case {
        name: name.to_string(),
        class: id.to_string(),
        properties: vec![
            ("tondo.id".into(), Value::Str(format!("{id}::{name}"))),
            ("tondo.synthetic".into(), Value::Str(kind.into())),
        ],
        outcome,
        stdout: decisive.stdout.clone(),
        stderr: decisive.stderr.clone(),
        time: if name == "@flaky" { Duration::ZERO } else { decisive.duration },
    }
}

/// A `testsuite` and its counts: tests, failures, errors, skipped, time.
fn testsuite(
    name: &str,
    properties: &[(String, Value)],
    cases: &[Case],
) -> (String, (usize, usize, usize, usize, Duration)) {
    let count = |element: &str| cases.iter().filter(|c| c.outcome.as_ref().is_some_and(|o| o.0 == element)).count();
    let time: Duration = cases.iter().map(|c| c.time).sum();
    let counts = (cases.len(), count("failure"), count("error"), count("skipped"), time);
    let mut xml = format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\" skipped=\"{}\" time=\"{}\">\n",
        attribute(name),
        counts.0,
        counts.1,
        counts.2,
        counts.3,
        seconds(time)
    );
    xml.push_str(&properties_xml(properties, "    "));
    for case in cases {
        xml.push_str(&format!(
            "    <testcase name=\"{}\" classname=\"{}\" time=\"{}\">\n",
            attribute(&case.name),
            attribute(&case.class),
            seconds(case.time)
        ));
        xml.push_str(&properties_xml(&case.properties, "      "));
        if let Some((element, kind, message, body)) = &case.outcome {
            let kind = if kind.is_empty() { String::new() } else { format!(" type=\"{}\"", attribute(kind)) };
            if body.is_empty() {
                xml.push_str(&format!("      <{element}{kind} message=\"{}\"/>\n", attribute(message)));
            } else {
                xml.push_str(&format!(
                    "      <{element}{kind} message=\"{}\">{}</{element}>\n",
                    attribute(message),
                    text(body)
                ));
            }
        }
        for (element, bytes) in [("system-out", &case.stdout), ("system-err", &case.stderr)] {
            if !bytes.is_empty() {
                xml.push_str(&format!("      <{element}>{}</{element}>\n", stream_text(bytes)));
            }
        }
        xml.push_str("    </testcase>\n");
    }
    xml.push_str("  </testsuite>\n");
    (xml, counts)
}

fn properties_xml(properties: &[(String, Value)], indent: &str) -> String {
    if properties.is_empty() {
        return String::new();
    }
    let mut xml = format!("{indent}<properties>\n");
    for (name, value) in properties {
        let value = match value {
            Value::Str(text) => text.clone(),
            other => other.text_for_xml(),
        };
        xml.push_str(&format!("{indent}  <property name=\"{name}\" value=\"{}\"/>\n", attribute(&value)));
    }
    xml.push_str(&format!("{indent}</properties>\n"));
    xml
}

/// A stream as text when XML can hold it unchanged, or as Base64.
fn stream_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(utf8) if !utf8.contains('\r') && utf8.chars().all(legal) => text(utf8),
        _ => json::base64(bytes),
    }
}

fn legal(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && !matches!(c, '\u{fffe}' | '\u{ffff}'))
}

/// Text for an element: escaped, with what XML cannot hold as `\u{HEX}`.
fn text(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if !legal(c) => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn attribute(value: &str) -> String {
    text(value).replace('"', "&quot;").replace('\n', "&#10;").replace('\t', "&#9;").replace('\r', "&#13;")
}

/// Seconds with up to nine decimals, without an exponent.
fn seconds(time: Duration) -> String {
    let text = format!("{}.{:09}", time.as_secs(), time.subsec_nanos());
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_string()
}
