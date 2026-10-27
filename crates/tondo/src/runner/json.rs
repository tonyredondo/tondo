//! The machine-readable report and list (testing 15), in the canonical
//! compact JSON of 15.1, and the values JUnit embeds.

use super::worker::Note;
use super::{Aggregate, Attempt, Options, Order, Selector, Status, Tree};

/// A JSON value whose objects keep the order they are built in.
#[derive(Clone)]
pub(super) enum Value {
    Null,
    Bool(bool),
    Int(i64),
    /// A number in the closed decimal form of its field, as written.
    Number(String),
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
        Value::Number(text) => out.push_str(text),
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

pub(super) fn str(text: impl Into<String>) -> Value {
    Value::Str(text.into())
}

pub(super) fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub(super) fn int(n: usize) -> Value {
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
    /// The snapshot tree's hash before and after the run, and whether an
    /// update was published.
    pub snapshots: (&'a str, &'a str, Option<bool>),
    pub compiled: bool,
    /// What the benchmarks compare with, and the regression allowed.
    pub baselines: &'a super::bench::Baselines,
    /// With `--profile`, each profile written: its test, benchmark and file.
    pub profiles: &'a [(String, Option<String>, std::path::PathBuf)],
}

impl Context<'_> {
    fn header(&self, format: &str, with_plan: bool) -> Vec<(&'static str, Value)> {
        let (kind, value) = match &self.options.selector {
            Selector::All => ("all", Value::Null),
            Selector::Filter(text) => ("filter", str(text.clone())),
            Selector::Glob(glob) => ("glob", str(glob.text())),
            Selector::Exact(ids) => match &ids[..] {
                [id] => ("exact", str(id.clone())),
                ids => ("exact", Value::Array(ids.iter().map(|id| str(id.clone())).collect())),
            },
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
            let store = obj(vec![("format", str("tondo-snapshot-store-0.1/2")), ("sha256", str(self.snapshots.0))]);
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
        let mut fields = vec![
            ("id", str(id)),
            ("parent", if parent == 0 { Value::Null } else { str(tree.id(parent)) }),
            ("package", str(parts[0])),
            ("kind", str(parts[1])),
            ("module", str(parts[2])),
            ("path", Value::Array(parts[3..].iter().map(|&p| str(p)).collect())),
            ("name", str(*parts.last().unwrap_or(&""))),
            ("source", source(Some(&tree.locations[node as usize]))),
            ("owners", Value::Array(self.owners[node as usize].iter().map(|o| str(o.clone())).collect())),
        ];
        // A test says whether it is a benchmark (10.4), and whether it
        // fuzzes (10.6).
        if !tree.suite(node) {
            fields.push(("bench", Value::Bool(tree.nodes[node as usize].bench)));
            fields.push(("fuzz", Value::Bool(tree.nodes[node as usize].fuzz)));
        }
        fields
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
            "schedules",
            match options.schedules {
                Some((count, seed, _)) => obj(vec![
                    ("count", int(count as usize)),
                    ("seed", str(format!("{seed:016x}"))),
                    ("algorithm", str("seeded-single-worker-v1")),
                ]),
                None => Value::Null,
            },
        ));
        fields.push((
            "artifact_store",
            obj(vec![("format", str("tondo-test-artifacts-0.1/1")), ("algorithm", str("sha256-objects-v1"))]),
        ));
        fields.push((
            "snapshot_policy",
            obj(vec![
                ("format", str("tondo-snapshot-store-0.1/2")),
                ("mode", str(if options.update_snapshots { "update" } else { "check" })),
                ("before_sha256", str(self.snapshots.0)),
                ("after_sha256", str(self.snapshots.1)),
                ("published", self.snapshots.2.map_or(Value::Null, Value::Bool)),
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
        let bench = options.bench.map(|time| {
            let Value::Object(mut fields) = super::bench::environment(time) else { unreachable!("an object") };
            fields.push(("baseline".into(), self.baselines.source.clone().unwrap_or(Value::Null)));
            let max = options.max_regression.map_or(Value::Null, |m| super::bench::decimal(m * 100.0));
            fields.push(("max_regression_percent".into(), max));
            Value::Object(fields)
        });
        fields.push(("bench", bench.unwrap_or(Value::Null)));
        let profiles = self.profiles.iter().map(|(test, bench, path)| {
            obj(vec![
                ("test", str(test.clone())),
                ("bench", bench.as_ref().map_or(Value::Null, |b| str(b.clone()))),
                ("profile", str(path.display().to_string())),
            ])
        });
        fields.push(("profiles", options.profile.map_or(Value::Null, |_| Value::Array(profiles.collect()))));
        let fuzz = options.fuzz.as_ref().map_or(Value::Null, |fuzz| {
            let corpus = fuzz.corpus.clone().unwrap_or_else(|| std::path::PathBuf::from("target/fuzz"));
            obj(vec![
                ("time_ms", fuzz.time.map_or(Value::Null, |t| int(t.as_millis() as usize))),
                ("timeout_ms", int(fuzz.timeout.as_millis() as usize)),
                ("memory_mib", int(fuzz.memory as usize)),
                ("corpus", str(corpus.display().to_string())),
            ])
        });
        fields.push(("fuzz", fuzz));
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
        fields.push(("skip", skip(&attempt.status)));
        let mut tags = attempt.tags.clone();
        tags.sort();
        fields.push(("tags", Value::Object(tags.into_iter().map(|(k, v)| (k, str(v))).collect())));
        fields.push(("artifacts", artifacts(attempt)));
        fields.push(("snapshots", snapshots(attempt)));
        fields.push(("virtual_time", virtual_time(attempt)));
        let max = self.options.max_regression;
        let id = self.tree.id(attempt.node);
        let benchmarks = attempt.benchmarks.iter().map(|b| {
            let base = self.baselines.measures.get(&(id.to_string(), b.name.clone()));
            super::bench::json(b, base.map(|base| super::bench::compare(b, base, max)).as_ref())
        });
        fields.push(("benchmarks", Value::Array(benchmarks.collect())));
        fields.push(("fuzz", Value::Array(attempt.fuzz.iter().map(fuzzed).collect())));
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
        let every: Vec<&Attempt> = aggregates.iter().flat_map(|a| &a.attempts).collect();
        let snapshot_count =
            |status: &str| int(every.iter().flat_map(|a| &a.snapshots).filter(|s| s.status == status).count());
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
            ("artifacts", int(every.iter().map(|a| a.artifacts.len()).sum())),
            ("artifact_bytes", str(every.iter().flat_map(|a| &a.artifacts).map(|a| a.size).sum::<usize>().to_string())),
            ("snapshots", int(every.iter().map(|a| a.snapshots.len()).sum())),
            ("snapshot_matched", snapshot_count("matched")),
            ("snapshot_missing", snapshot_count("missing")),
            ("snapshot_mismatched", snapshot_count("mismatched")),
            ("snapshot_created", snapshot_count("created")),
            ("snapshot_updated", snapshot_count("updated")),
            ("failed", int(test_failed + suite_failed)),
        ])
    }
}

/// The virtual clocks of an attempt, in the order they ran.
pub(super) fn virtual_time(attempt: &Attempt) -> Value {
    let clocks = attempt.virtual_time.iter().enumerate().map(|(index, c)| {
        obj(vec![
            ("index", int(index + 1)),
            ("elapsed_ns", str(c.elapsed_ns.to_string())),
            ("automatic_advances", int(c.automatic as usize)),
            ("explicit_advances", int(c.explicit as usize)),
            ("settles", int(c.settles as usize)),
        ])
    });
    Value::Array(clocks.collect())
}

/// An attempt's attachments, by name.
pub(super) fn artifacts(attempt: &Attempt) -> Value {
    let mut list: Vec<&super::Artifact> = attempt.artifacts.iter().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Value::Array(
        list.into_iter()
            .map(|a| {
                obj(vec![
                    ("name", str(a.name.clone())),
                    ("media_type", str(a.media_type.clone())),
                    ("size", str(a.size.to_string())),
                    ("sha256", str(a.sha256.clone())),
                    ("object", str(format!("objects/{}", a.sha256))),
                ])
            })
            .collect(),
    )
}

/// An attempt's snapshot checks, by name.
pub(super) fn snapshots(attempt: &Attempt) -> Value {
    let mut list: Vec<&super::Snapshot> = attempt.snapshots.iter().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Value::Array(
        list.into_iter()
            .map(|s| {
                obj(vec![
                    ("name", str(s.name.clone())),
                    ("status", str(s.status.clone())),
                    (
                        "expected_sha256",
                        s.expected.as_ref().map_or(Value::Null, |e| str(crate::hash::sha256_hex(e.as_bytes()))),
                    ),
                    ("actual_sha256", str(crate::hash::sha256_hex(s.actual.as_bytes()))),
                ])
            })
            .collect(),
    )
}

pub(super) fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Skipped(_) => "skipped",
        Status::Error { .. } => "failed-error",
        Status::Panic(_) => "failed-panic",
        Status::Timeout | Status::Limit { timeout: true, .. } => "timeout",
        Status::Limit { timeout: false, .. } => "resource-limit",
        Status::Crashed(_) => "crashed",
        Status::Blocked(_, false) => "blocked-setup",
        Status::Blocked(_, true) => "blocked-skip",
    }
}

/// The report as stdout gets it: alone, or as the last event (15.9).
pub(super) fn reported(report: &Value, options: &Options) -> String {
    match options.events {
        true => obj(vec![("event", str("report")), ("report", report.clone())]).text(),
        false => report.text(),
    }
}

/// The event of a pass starting (15.9): an iteration of the plan or a
/// retry round, with the tests it runs.
pub(super) fn pass_event(tree: &Tree, iteration: u32, round: u32, tests: &[u32]) -> Value {
    obj(vec![
        ("event", str("pass")),
        ("format", str("tondo-test-events-0.1/1")),
        ("iteration", int(iteration as usize)),
        ("round", int(round as usize)),
        ("tests", Value::Array(tests.iter().map(|&t| str(tree.id(t))).collect())),
    ])
}

/// The event of a suite or test starting, writing or ending (15.9).
pub(super) fn note_event(tree: &Tree, note: &Note, options: &Options) -> Value {
    let (event, node, iteration, round) = match note {
        Note::Started(node, context) => ("start", *node, context.iteration, context.round),
        Note::Wrote(node, context, ..) => ("output", *node, context.iteration, context.round),
        Note::Ended(attempt) => ("end", attempt.node, attempt.iteration, attempt.round),
        Note::Fuzzing(node, ..) => ("fuzz", *node, 1, 0),
    };
    let mut fields = vec![
        ("event", str(event)),
        ("id", str(tree.id(node))),
        ("suite", Value::Bool(tree.suite(node))),
        ("iteration", int(iteration as usize)),
        ("round", int(round as usize)),
    ];
    if let Note::Wrote(_, _, stdout, text) = note {
        fields.push((if *stdout { "stdout" } else { "stderr" }, stream(text)));
    }
    if let Note::Ended(attempt) = note {
        fields.extend([
            ("status", str(status_name(&attempt.status))),
            ("duration_ms", int(attempt.duration.as_millis() as usize)),
            ("failure", failure(&attempt.status, options)),
            ("skip", skip(&attempt.status)),
            ("logs", Value::Array(attempt.logs.iter().map(|l| str(l.clone())).collect())),
            ("stdout", stream(&attempt.stdout)),
            ("stderr", stream(&attempt.stderr)),
            ("benchmarks", Value::Array(attempt.benchmarks.iter().map(|b| super::bench::json(b, None)).collect())),
            ("fuzz", Value::Array(attempt.fuzz.iter().map(fuzzed).collect())),
        ]);
    }
    if let Note::Fuzzing(_, name, searched) = note {
        fields.push(("name", str(name.clone())));
        fields.extend(searched_fields(searched));
    }
    obj(fields)
}

/// What a search did (15.15): the inputs it tried, for how long, its
/// corpus, the counters reached and those there are, and how long since it
/// found something new.
fn searched_fields(searched: &super::Searched) -> Vec<(&'static str, Value)> {
    vec![
        ("inputs", int(searched.inputs as usize)),
        ("elapsed_ms", int(searched.elapsed_ms as usize)),
        ("corpus", int(searched.corpus as usize)),
        ("coverage", obj(vec![("reached", int(searched.reached as usize)), ("total", int(searched.total as usize))])),
        ("new_ms", int(searched.new_ms as usize)),
    ]
}

/// Why an attempt skipped, and where, or `null`.
fn skip(status: &Status) -> Value {
    match status {
        Status::Skipped(text) => {
            let (reason, location) = located(text);
            obj(vec![("reason", str(reason)), ("source", source(location))])
        }
        _ => Value::Null,
    }
}

/// The failure of an attempt (15.3), or `null`.
pub(super) fn failure(status: &Status, options: &Options) -> Value {
    let mut stack = Vec::new();
    let (kind, code, error_type, message, location) = match status {
        Status::Error { message, error_type, location } => {
            ("error", None, Some(error_type.clone()), message.clone(), location.clone())
        }
        Status::Panic(text) => {
            // `panic[P0007]: name: message`, then ` --> location` and the
            // stack trace.
            let (first, location) = located(text);
            let code = first.strip_prefix("panic[").and_then(|r| r.split_once(']')).map(|(c, _)| c.to_string());
            let message = first.split_once("]: ").map_or(first, |(_, m)| m).to_string();
            stack = frames(text);
            ("panic", code, None, message, location.map(str::to_string))
        }
        Status::Timeout => ("timeout", None, None, format!("timed out after {}s", options.timeout.as_secs_f64()), None),
        Status::Crashed(how) => ("crashed", None, None, how.clone(), None),
        Status::Limit { timeout, message } => {
            (if *timeout { "timeout" } else { "resource-limit" }, None, None, message.clone(), None)
        }
        _ => return Value::Null,
    };
    // The sides of a comparison asserted, as language 15.8 shows them.
    let comparison = match message.split_once("\n  left:  ").and_then(|(_, rest)| rest.split_once("\n  right: ")) {
        Some((left, right)) if code.as_deref() == Some("P0007") => {
            obj(vec![("left", str(left)), ("right", str(right))])
        }
        _ => Value::Null,
    };
    obj(vec![
        ("kind", str(kind)),
        ("code", opt(code.as_deref())),
        ("error_type", opt(error_type.as_deref())),
        ("message", str(message)),
        ("source", source(location.as_deref())),
        ("comparison", comparison),
        ("stack", Value::Array(stack)),
    ])
}

/// The frames of a panic's stack trace: each `  function  file:line:column`
/// line, maybe with `  (n times)`, with `task` 0 for the stack of the task
/// that panicked and one more for each `in a task spawned at:` before it.
fn frames(text: &str) -> Vec<Value> {
    let mut task = None;
    let mut frames = Vec::new();
    for line in text.lines() {
        match line {
            "stack:" => task = Some(0),
            "in a task spawned at:" => task = Some(task.map_or(1, |t| t + 1)),
            _ if line.starts_with("  ") => {
                let Some(task) = task else { continue };
                let (line, times) = match line.strip_suffix(" times)").and_then(|l| l.rsplit_once("  (")) {
                    Some((line, times)) => (line, times.parse().unwrap_or(1)),
                    None => (line, 1),
                };
                let Some((function, location)) = line.trim_start().rsplit_once("  ") else { continue };
                frames.push(obj(vec![
                    ("function", str(function.trim_end())),
                    ("source", source(Some(location))),
                    ("task", Value::Int(task)),
                    ("times", Value::Int(times)),
                ]));
            }
            _ => {}
        }
    }
    frames
}

/// What a call of `testing.fuzz` did, as an attempt's `fuzz` says it
/// (15.15).
fn fuzzed(fuzzed: &super::Fuzzed) -> Value {
    let input = fuzzed.input.as_ref().map_or(Value::Null, |input| {
        obj(vec![
            ("json", str(input.json.clone())),
            ("file", input.file.clone().map_or(Value::Null, str)),
            ("seed", input.seed.map_or(Value::Null, |s| int(s as usize))),
            ("reduced_from", input.reduced_from.map_or(Value::Null, int)),
        ])
    });
    let search = fuzzed.search.as_ref().map_or(Value::Null, |searched| {
        let mut fields = searched_fields(searched);
        fields.retain(|(key, _)| !matches!(*key, "inputs" | "new_ms"));
        obj(fields)
    });
    obj(vec![
        ("name", str(fuzzed.name.clone())),
        ("inputs", int(fuzzed.inputs as usize)),
        ("search", search),
        ("input", input),
    ])
}

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
