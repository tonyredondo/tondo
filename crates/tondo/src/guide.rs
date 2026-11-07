//! `tondo guide` (toolchain 7.3): a guide to Tondo for people and models,
//! a topic at a time, carried by the toolchain so that it matches it; a test
//! compiles every example.

/// The topics, in the order the overview lists them.
pub const TOPICS: &[(&str, &str)] = &[
    ("overview", include_str!("../guide/overview.md")),
    ("syntax", include_str!("../guide/syntax.md")),
    ("types", include_str!("../guide/types.md")),
    ("errors", include_str!("../guide/errors.md")),
    ("collections", include_str!("../guide/collections.md")),
    ("text", include_str!("../guide/text.md")),
    ("tasks", include_str!("../guide/tasks.md")),
    ("projects", include_str!("../guide/projects.md")),
    ("testing", include_str!("../guide/testing.md")),
    ("models", include_str!("../guide/models.md")),
    ("stdlib", include_str!("../guide/stdlib.md")),
    ("http", include_str!("../guide/http.md")),
    ("templates", include_str!("../guide/templates.md")),
    ("databases", include_str!("../guide/databases.md")),
    ("meta", include_str!("../guide/meta.md")),
    ("ffi", include_str!("../guide/ffi.md")),
    ("cli", include_str!("../guide/cli.md")),
    ("mcp", include_str!("../guide/mcp.md")),
    ("pitfalls", include_str!("../guide/pitfalls.md")),
    ("runbooks", include_str!("../guide/runbooks.md")),
    ("runbook-crash", include_str!("../guide/runbook-crash.md")),
    ("runbook-regression", include_str!("../guide/runbook-regression.md")),
    ("runbook-hang", include_str!("../guide/runbook-hang.md")),
    ("runbook-live", include_str!("../guide/runbook-live.md")),
    ("runbook-compile", include_str!("../guide/runbook-compile.md")),
    ("runbook-flaky", include_str!("../guide/runbook-flaky.md")),
    ("runbook-debug", include_str!("../guide/runbook-debug.md")),
    ("runbook-slow-request", include_str!("../guide/runbook-slow-request.md")),
    ("runbook-performance", include_str!("../guide/runbook-performance.md")),
    ("runbook-memory", include_str!("../guide/runbook-memory.md")),
    ("runbook-production", include_str!("../guide/runbook-production.md")),
    ("runbook-fuzz", include_str!("../guide/runbook-fuzz.md")),
];

pub fn topic(name: &str) -> Option<&'static str> {
    TOPICS.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}

/// The overview as an agent skill: a `SKILL.md` with its front matter.
pub fn skill() -> String {
    let description = "Write, check, run and test Tondo programs (.to files, tondo.toml projects): \
the language's rules, its standard library and the tondo commands, including `tondo api` to see what a \
module offers and `tondo guide <topic>` for more; and setting Tondo up for agents and editors, with `tondo setup`.";
    format!("---\nname: tondo\ndescription: {description}\n---\n\n{}", TOPICS[0].1)
}
