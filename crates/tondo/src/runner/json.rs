//! The machine-readable report and list (testing 15), in the canonical
//! compact JSON of 15.1, and the values JUnit embeds.

use super::{Aggregate, Attempt, Options, Order, Selector, Status, Tree};

/// A JSON value whose objects keep the order they are built in.
#[derive(Clone)]
pub(super) enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub(super) fn text(&self) -> String {
        let mut out = String::new();
        write(self, false, &mut out);
        out
    }

    /// For XML: also escapes U+FFFE and U+FFFF, which XML cannot hold.
    pub(super) fn text_for_xml(&self) -> String {
        let mut out = String::new();
        write(self, true, &mut out);
        out
    }

    pub(super) fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

fn write(value: &Value, xml: bool, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Str(text) => string(text, xml, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(item, xml, out);
            }
            out.push(']');
        }
        Value::Object(fields) => {
            out.push('{');
            for (index, (key, item)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                string(key, xml, out);
                out.push(':');
                write(item, xml, out);
            }
            out.push('}');
        }
    }
}

fn string(text: &str, xml: bool, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || (xml && matches!(c, '\u{fffe}' | '\u{ffff}')) => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn str(text: impl Into<String>) -> Value {
    Value::Str(text.into())
}

fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn int(n: usize) -> Value {
    Value::Int(n as i64)
}

fn opt(text: Option<&str>) -> Value {
    text.map_or(Value::Null, str)
}

/// Standard padded Base64.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |w, (i, &b)| w | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(word >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A stream as `{encoding, data}` (9.3): UTF-8 when it is, Base64 if not.
pub(super) fn stream(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => obj(vec![("encoding", str("utf8")), ("data", str(text))]),
        Err(_) => obj(vec![("encoding", str("base64")), ("data", str(base64(bytes)))]),
    }
}

/// `file:line:column` as a source object, or `null`.
fn source(location: Option<&str>) -> Value {
    let Some(location) = location else { return Value::Null };
    let mut parts = location.rsplitn(3, ':');
    let (column, line, file) = (parts.next(), parts.next(), parts.next());
    match (file, line.and_then(|l| l.parse().ok()), column.and_then(|c| c.parse().ok())) {
        (Some(file), Some(line), Some(column)) => {
            obj(vec![("file", str(file)), ("line", Value::Int(line)), ("column", Value::Int(column))])
        }
        _ => Value::Null,
    }
}

/// Splits `text\n --> location` into the text and the location.
fn located(text: &str) -> (&str, Option<&str>) {
    match text.split_once("\n --> ") {
        Some((text, rest)) => (text, Some(rest.lines().next().unwrap_or(rest))),
        None => (text, None),
    }
}

/// What the report knows besides the attempts.
pub(super) struct Context<'a> {
    pub tree: &'a Tree,
    pub options: &'a Options,
    pub owners: &'a [Vec<String>],
    pub ownership: &'a super::owners::Owners,
    pub plan: &'a [u32],
    /// Each retry round that ran: its units, each with its tests.
    pub rounds: &'a [Vec<(u32, Vec<u32>)>],
    pub snapshot_sha256: &'a str,
    pub compiled: bool,
}

impl Context<'_> {
    fn header(&self, format: &str, with_plan: bool) -> Vec<(&'static str, Value)> {
        let (kind, value) = match &self.options.selector {
            Selector::All => ("all", Value::Null),
            Selector::Filter(text) => ("filter", str(text.clone())),
            Selector::Glob(glob) => ("glob", str(glob.text())),
            Selector::Exact(id) => ("exact", str(id.clone())),
        };
        let mode = match self.options.codeowners {
            super::Codeowners::Auto => "auto",
            super::Codeowners::None => "none",
            super::Codeowners::Path(_) => "explicit",
        };
        let shard = match self.options.shard {
            Some((index, count)) => obj(vec![
                ("index", Value::Int(index as i64)),
                ("count", Value::Int(count as i64)),
                ("algorithm", str("sha256-mod-v1")),
            ]),
            None => Value::Null,
        };
        let order = match self.options.order {
            Order::Canonical => {
                obj(vec![("mode", str("canonical")), ("seed", Value::Null), ("algorithm", str("id-byte-order-v1"))])
            }
            Order::Random(seed, _) => obj(vec![
                ("mode", str("random")),
                ("seed", str(format!("{seed:016x}"))),
                ("algorithm", str("sha256-tree-v1")),
            ]),
        };
        let mut fields = vec![
            ("format", str(format)),
            ("edition", str("0.1")),
            (
                "target",
                obj(vec![
                    ("name", str(format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS))),
                    ("profile", str("hosted")),
                ]),
            ),
            ("compiled", Value::Bool(self.compiled)),
            ("selection", obj(vec![("kind", str(kind)), ("value", value)])),
            (
                "ownership",
                obj(vec![
                    ("mode", str(mode)),
                    ("source", opt(self.ownership.source.as_deref())),
                    ("sha256", opt(self.ownership.sha256.as_deref())),
                ]),
            ),
        ];
        if !with_plan {
            let store = obj(vec![("format", str("tondo-snapshot-store-0.1/1")), ("sha256", str(self.snapshot_sha256))]);
            fields.push(("snapshot_store", store));
        }
        fields.push(("shard", shard));
        fields.push(("order", order));
        fields.push(("execution_plan", Value::Array(self.plan.iter().map(|&t| str(self.tree.id(t))).collect())));
        fields
    }

    /// A node's descriptor: identity, place, declaration and owners.
    fn descriptor(&self, node: u32) -> Vec<(&'static str, Value)> {
        let tree = self.tree;
        let id = tree.id(node);
        let parts: Vec<&str> = id.split("::").collect();
        let parent = tree.nodes[node as usize].parent;
        vec![
            ("id", str(id)),
            ("parent", if parent == 0 { Value::Null } else { str(tree.id(parent)) }),
            ("package", str(parts[0])),
            ("kind", str(parts[1])),
            ("module", str(parts[2])),
            ("path", Value::Array(parts[3..].iter().map(|&p| str(p)).collect())),
            ("name", str(*parts.last().unwrap_or(&""))),
            ("source", source(Some(&tree.locations[node as usize]))),
            ("owners", Value::Array(self.owners[node as usize].iter().map(|o| str(o.clone())).collect())),
        ]
    }

    pub(super) fn descriptor_fields(&self, node: u32) -> Vec<(String, Value)> {
        self.descriptor(node).into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    /// The suites and tests of the plan, each in canonical order.
    fn forest(&self) -> (Vec<u32>, Vec<u32>) {
        let mut suites: Vec<u32> = Vec::new();
        for &test in self.plan {
            let mut node = self.tree.nodes[test as usize].parent;
            while node != 0 {
                if !suites.contains(&node) {
                    suites.push(node);
                }
                node = self.tree.nodes[node as usize].parent;
            }
        }
        let mut tests = self.plan.to_vec();
        suites.sort_by(|&a, &b| self.tree.id(a).cmp(self.tree.id(b)));
        tests.sort_by(|&a, &b| self.tree.id(a).cmp(self.tree.id(b)));
        (suites, tests)
    }

    /// `tondo-test-list-0.1/1` (15.4).
    pub(super) fn list(&self) -> Value {
        let mut fields = self.header("tondo-test-list-0.1/1", false);
        let (suites, tests) = self.forest();
        fields.push(("suites", Value::Array(suites.iter().map(|&s| obj(self.descriptor(s))).collect())));
        fields.push(("tests", Value::Array(tests.iter().map(|&t| obj(self.descriptor(t))).collect())));
        obj(fields)
    }

    /// `tondo-test-report-0.1/1` (15.1).
    pub(super) fn report(&self, aggregates: &[Aggregate]) -> Value {
        let options = self.options;
        let mut fields = self.header("tondo-test-report-0.1/1", true);
        let rounds: Vec<Value> = self
            .rounds
            .iter()
            .enumerate()
            .map(|(index, units)| {
                let units = units
                    .iter()
                    .map(|(unit, tests)| {
                        obj(vec![
                            ("kind", str(if self.tree.suite(*unit) { "suite" } else { "test" })),
                            ("id", str(self.tree.id(*unit))),
                            ("execution_plan", Value::Array(tests.iter().map(|&t| str(self.tree.id(t))).collect())),
                        ])
                    })
                    .collect();
                obj(vec![("round", int(index + 1)), ("units", Value::Array(units))])
            })
            .collect();
        fields.push((
            "retry",
            obj(vec![
                ("max_additional_rounds", int(options.retry as usize)),
                ("isolation", str("fresh-worker-v1")),
                ("rounds", Value::Array(rounds)),
            ]),
        ));
        fields.push((
            "repeat",
            obj(vec![("count", int(options.repeat as usize)), ("isolation", str("fresh-worker-per-iteration-v1"))]),
        ));
        fields.push((
            "artifact_store",
            obj(vec![("format", str("tondo-test-artifacts-0.1/1")), ("algorithm", str("sha256-objects-v1"))]),
        ));
        fields.push((
            "snapshot_policy",
            obj(vec![
                ("format", str("tondo-snapshot-store-0.1/1")),
                ("mode", str("check")),
                ("before_sha256", str(self.snapshot_sha256)),
                ("after_sha256", str(self.snapshot_sha256)),
                ("published", Value::Null),
            ]),
        ));
        fields.push((
            "policy",
            obj(vec![
                ("deny_skips", Value::Bool(options.deny_skips)),
                ("allow_flaky", Value::Bool(options.allow_flaky)),
            ]),
        ));
        fields.push((
            "limits",
            obj(vec![("jobs", int(options.jobs)), ("timeout_ms", int(options.timeout.as_millis() as usize))]),
        ));
        let (suites, tests) = self.forest();
        let find = |node: u32| aggregates.iter().find(|a| a.node == node);
        let nodes = |list: &[u32]| {
            Value::Array(list.iter().filter_map(|&n| find(n)).map(|a| obj(self.node(a, aggregates))).collect())
        };
        fields.push(("suites", nodes(&suites)));
        fields.push(("tests", nodes(&tests)));
        fields.push(("summary", self.summary(aggregates, &suites, &tests)));
        obj(fields)
    }

    fn node(&self, aggregate: &Aggregate, all: &[Aggregate]) -> Vec<(&'static str, Value)> {
        let mut fields = self.descriptor(aggregate.node);
        let status =
            if aggregate.flaky { "flaky-pass".to_string() } else { status_name(&aggregate.status).to_string() };
        fields.push(("status", str(status)));
        fields.push(("decisive_attempt", int(aggregate.decisive + 1)));
        let attempts =
            aggregate.attempts.iter().enumerate().map(|(index, a)| self.attempt(index + 1, a, all)).collect();
        fields.push(("attempts", Value::Array(attempts)));
        fields
    }

    pub(super) fn attempt(&self, index: usize, attempt: &Attempt, all: &[Aggregate]) -> Value {
        let suite = self.tree.suite(attempt.node);
        // A retry unit by its place in its round.
        let unit = match (attempt.round, attempt.unit) {
            (round, Some(unit)) if round > 0 => {
                self.rounds[round as usize - 1].iter().position(|(u, _)| *u == unit).map_or(Value::Null, |p| int(p + 1))
            }
            _ => Value::Null,
        };
        let mut fields = vec![
            ("index", int(index)),
            ("iteration", int(attempt.iteration as usize)),
            ("round", int(attempt.round as usize)),
            ("unit", unit),
            ("status", str(status_name(&attempt.status))),
        ];
        if suite {
            let phase = match &attempt.status {
                Status::Passed | Status::Blocked(..) => Value::Null,
                Status::Skipped(_) => str("setup"),
                _ => str(if attempt.teardown { "teardown" } else { "setup" }),
            };
            fields.push(("phase", phase));
        }
        let blocked_by = match attempt.status {
            Status::Blocked(cause, skip) => {
                // The attempt of the suite that blocked it, in the same pass.
                let causes =
                    |s: &Attempt| if skip { matches!(s.status, Status::Skipped(_)) } else { s.status.failed() };
                let index = all.iter().find(|a| a.node == cause).and_then(|a| {
                    a.attempts.iter().rposition(|s| {
                        (s.iteration, s.round, s.unit) == (attempt.iteration, attempt.round, attempt.unit) && causes(s)
                    })
                });
                obj(vec![("id", str(self.tree.id(cause))), ("attempt", index.map_or(Value::Null, |i| int(i + 1)))])
            }
            _ => Value::Null,
        };
        fields.push(("blocked_by", blocked_by));
        fields.push(("failure", failure(&attempt.status, self.options)));
        let skip = match &attempt.status {
            Status::Skipped(text) => {
                let (reason, location) = located(text);
                obj(vec![("reason", str(reason)), ("source", source(location))])
            }
            _ => Value::Null,
        };
        fields.push(("skip", skip));
        let mut tags = attempt.tags.clone();
        tags.sort();
        fields.push(("tags", Value::Object(tags.into_iter().map(|(k, v)| (k, str(v))).collect())));
        fields.push(("artifacts", Value::Array(Vec::new())));
        fields.push(("snapshots", Value::Array(Vec::new())));
        fields.push(("virtual_time", Value::Array(Vec::new())));
        fields.push(("logs", Value::Array(attempt.logs.iter().map(|l| str(l.clone())).collect())));
        fields.push(("stdout", stream(&attempt.stdout)));
        fields.push(("stderr", stream(&attempt.stderr)));
        obj(fields)
    }

    fn summary(&self, aggregates: &[Aggregate], suites: &[u32], tests: &[u32]) -> Value {
        let of = |list: &[u32]| -> Vec<&Aggregate> { aggregates.iter().filter(|a| list.contains(&a.node)).collect() };
        let (suites, tests) = (of(suites), of(tests));
        let count = |list: &[&Aggregate], f: &dyn Fn(&Aggregate) -> bool| int(list.iter().filter(|a| f(a)).count());
        let is = |name: &'static str| move |a: &Aggregate| !a.flaky && status_name(&a.status) == name;
        let attempts = |list: &[&Aggregate]| int(list.iter().map(|a| a.attempts.len()).sum());
        let retried = |a: &Aggregate| a.attempts.iter().any(|t| t.round > 0);
        let repeated = |a: &Aggregate| a.attempts.iter().any(|t| t.iteration > 1);
        let failed = |a: &Aggregate| a.status.failed();
        let test_failed = tests.iter().filter(|a| failed(a)).count();
        let suite_failed = suites.iter().filter(|a| failed(a)).count();
        obj(vec![
            ("selected", int(tests.len())),
            ("executed", count(&tests, &|a| a.attempts.iter().any(|t| !matches!(t.status, Status::Blocked(..))))),
            ("passed", count(&tests, &is("passed"))),
            ("flaky_passed", count(&tests, &|a| a.flaky)),
            ("skipped", count(&tests, &is("skipped"))),
            ("blocked_setup", count(&tests, &is("blocked-setup"))),
            ("blocked_skip", count(&tests, &is("blocked-skip"))),
            ("failed_error", count(&tests, &is("failed-error"))),
            ("failed_panic", count(&tests, &is("failed-panic"))),
            ("resource_limit", count(&tests, &is("resource-limit"))),
            ("timeout", count(&tests, &is("timeout"))),
            ("crashed", count(&tests, &is("crashed"))),
            ("infrastructure", count(&tests, &is("infrastructure"))),
            ("retried", count(&tests, &retried)),
            ("repeated", count(&tests, &repeated)),
            ("test_attempts", attempts(&tests)),
            ("suite_selected", int(suites.len())),
            ("suite_passed", count(&suites, &is("passed"))),
            ("suite_flaky_passed", count(&suites, &|a| a.flaky)),
            ("suite_skipped", count(&suites, &is("skipped"))),
            ("suite_blocked_setup", count(&suites, &is("blocked-setup"))),
            ("suite_blocked_skip", count(&suites, &is("blocked-skip"))),
            ("suite_failed", int(suite_failed)),
            ("suite_retried", count(&suites, &retried)),
            ("suite_repeated", count(&suites, &repeated)),
            ("suite_attempts", attempts(&suites)),
            ("artifacts", int(0)),
            ("artifact_bytes", str("0")),
            ("snapshots", int(0)),
            ("snapshot_matched", int(0)),
            ("snapshot_missing", int(0)),
            ("snapshot_mismatched", int(0)),
            ("snapshot_created", int(0)),
            ("snapshot_updated", int(0)),
            ("failed", int(test_failed + suite_failed)),
        ])
    }
}

pub(super) fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Skipped(_) => "skipped",
        Status::Error { .. } => "failed-error",
        Status::Panic(_) => "failed-panic",
        Status::Timeout => "timeout",
        Status::Crashed(_) => "crashed",
        Status::Blocked(_, false) => "blocked-setup",
        Status::Blocked(_, true) => "blocked-skip",
    }
}

/// The failure of an attempt (15.3), or `null`.
pub(super) fn failure(status: &Status, options: &Options) -> Value {
    let (kind, code, error_type, message, location) = match status {
        Status::Error { message, error_type, location } => {
            ("error", None, Some(error_type.clone()), message.clone(), location.clone())
        }
        Status::Panic(text) => {
            // `panic[P0007]: name: message`, then ` --> location`.
            let (first, location) = located(text);
            let code = first.strip_prefix("panic[").and_then(|r| r.split_once(']')).map(|(c, _)| c.to_string());
            let message = first.split_once("]: ").map_or(first, |(_, m)| m).to_string();
            ("panic", code, None, message, location.map(str::to_string))
        }
        Status::Timeout => ("timeout", None, None, format!("timed out after {}s", options.timeout.as_secs_f64()), None),
        Status::Crashed(how) => ("crashed", None, None, how.clone(), None),
        _ => return Value::Null,
    };
    obj(vec![
        ("kind", str(kind)),
        ("code", opt(code.as_deref())),
        ("error_type", opt(error_type.as_deref())),
        ("message", str(message)),
        ("source", source(location.as_deref())),
        ("stack", Value::Array(Vec::new())),
    ])
}

/// The canonical bytes of an empty snapshot store (15.7).
pub(super) const EMPTY_STORE: &str = "{\n  \"format\": \"tondo-snapshot-store-0.1/1\",\n  \"entries\": []\n}\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_canonical_json() {
        let value = obj(vec![("a", str("q\"\\\n\u{1}/é")), ("b", Value::Array(vec![Value::Null, Value::Bool(true)]))]);
        assert_eq!(value.text(), "{\"a\":\"q\\\"\\\\\\n\\u0001/é\",\"b\":[null,true]}");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
