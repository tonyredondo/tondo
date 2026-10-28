//! `tondo explain <code>` (toolchain 7.13): what a code of a diagnostic or
//! a panic means, from the registry of language specification 22.2 and the
//! testing specification 14, with the examples of it the guide has.

/// Each code: its stable name, and the condition it reports.
pub const CODES: &[(&str, &str, &str)] = &[
    ("E0001", "invalid-utf8", "A source file has a byte that is not valid UTF-8."),
    ("E0002", "invalid-token", "A character or sequence that is no token of the language."),
    ("E0003", "malformed-literal", "A number, string, character or interpolation literal is malformed."),
    ("E0004", "invalid-syntax", "The tokens do not form what the grammar requires there."),
    ("E0005", "invalid-operator-chain", "A chain of operators the language forbids, such as a chained comparison."),
    ("E0006", "invalid-source-form", "A file mixes, or is used as, the wrong form: module, script or snippet."),
    ("E1001", "unknown-name", "No visible declaration has the name."),
    ("E1002", "duplicate-name", "Two declarations take the same name in the same namespace and scope."),
    ("E1003", "shadowing", "A declaration would hide a binding that is still visible."),
    ("E1004", "ambiguous-name", "More than one visible declaration could be meant, and the name does not say which."),
    ("E1005", "reserved-name", "A declaration takes a contextual or intrinsic name the language reserves."),
    ("E1006", "import-cycle", "Modules import each other in a cycle."),
    ("E1007", "invalid-import-position", "An import comes after another declaration or a statement."),
    ("E1008", "invalid-module-path", "An import's path names no module available to the program."),
    ("E1101", "missing-type-context", "A type cannot be inferred in one way only, as of an empty literal."),
    ("E1102", "type-mismatch", "A value's type is not the exact type expected there."),
    ("E1103", "invalid-conversion", "A conversion the language does not allow."),
    ("E1104", "invalid-generic-arguments", "Generic arguments in the wrong number, order or binder."),
    ("E1105", "unsatisfied-bound", "A type does not implement a trait, capability or protocol it must."),
    ("E1106", "recursive-alias", "Transparent aliases refer to each other in a cycle."),
    ("E1107", "nonproductive-type", "A recursive nominal type has no finite base case."),
    ("E1108", "invalid-function-coercion", "A closure cannot become the `fn(...)` type expected."),
    ("E1109", "uninitialized-binding", "A binding is declared without a value."),
    ("E1110", "trait-is-not-a-value", "A trait is used as the type of a value outside a constraint."),
    ("E1111", "overlapping-impl", "Two `impl` headers apply to the same types."),
    ("E1112", "nonterminating-trait-resolution", "Trait bounds that refer to each other without ever shrinking."),
    ("E1113", "invalid-iterator-implementation", "A type would be an `Iterator` of more than one element type."),
    ("E1114", "invalid-impl-contract", "An `impl` does not match its trait exactly, or breaks the orphan rules."),
    ("E1115", "invalid-declaration", "A declaration is malformed in a way no more specific code names."),
    ("E1116", "duplicate-map-key", "A map literal repeats a constant key."),
    ("E1117", "invalid-opaque-result", "An opaque result has no single concrete type, or breaks its bounds."),
    ("E1201", "refutable-binding-pattern", "A pattern that can fail is used where one that cannot is required."),
    ("E1202", "invalid-pattern", "A pattern's form does not fit the value matched or its place."),
    ("E1203", "unreachable-match-arm", "An arm of `match` is covered by the arms before it."),
    ("E1204", "non-exhaustive-match", "The arms of `match` do not cover every case."),
    ("E1205", "invalid-control-transfer", "`return`, `break`, `continue` or `fail` has nowhere to go."),
    ("E1206", "invalid-for-source", "A `for` loop's header, pattern or source does not allow what it asks."),
    ("E1301", "incompatible-error-propagation", "`?` cannot pass on its error through the function's result."),
    ("E1302", "invalid-fail-context", "`fail` where no compatible error can leave."),
    ("E1303", "discarded-result", "A `Result` or `Option` is dropped as a statement, unused and without `_ =`."),
    (
        "E1304",
        "invalid-result-construction",
        "`ok`, `err`, `some` or `none` without a single context or a valid payload.",
    ),
    ("E1305", "invalid-panic-code", "The literal `code` of `panic` is not a code of the program's own."),
    ("E1401", "use-after-move", "A binding is used after its value moved away."),
    ("E1402", "invalid-borrow-lifetime", "A loan escapes, outlives what it lends, or crosses a boundary it may not."),
    ("E1403", "overlapping-mutable-borrow", "Loans that cannot overlap provably do."),
    ("E1405", "duplicate-assignment-destination", "Destinations that must differ overlap."),
    (
        "E1406",
        "invalid-affine-transfer",
        "A partial, duplicated or unconfirmable move of a value that cannot be copied.",
    ),
    ("E1407", "invalid-call-mode", "A callable needs `CallMut` or `CallOnce`, which its place does not allow."),
    ("E1410", "invalid-defer", "A `defer` that is not a call or a block, lends, returns or can fail."),
    ("E1411", "invalid-assignment-target", "What is assigned to does not allow the write the assignment needs."),
    ("E1501", "inaccessible-symbol", "A declaration or field exists but is not visible from this module."),
    ("E1502", "invalid-public-construction", "A construction from outside names or sets private state."),
    ("E1503", "private-type-in-public-api", "A public signature shows a type that is not public."),
    ("E1504", "invalid-method-owner", "This module cannot declare an inherent method of that type."),
    ("E1505", "member-name-conflict", "A field, method or variant breaks the uniqueness of its type's members."),
    ("E1605", "non-send-transfer", "A value that is not `Send` enters a task or goes through a channel."),
    ("E1609", "loan-to-task", "`spawn` is given an argument or receiver lent with `ref`, `mut` or `var`."),
    ("E1611", "invalid-spawn-operand", "`spawn` is not followed by a call."),
    ("E1612", "invalid-select-operand", "An arm of `select` is not `receive`, `send`, `wait`, `next` or `sleep`."),
    ("E1613", "invalid-select-shape", "`select` is empty, repeats `else`, puts it before an arm, or binds refutably."),
    ("E1615", "conflicting-select-ownership", "Two arms of `select` take the same owner in incompatible ways."),
    (
        "E1616",
        "invalid-context-declaration",
        "A field of a context has no constant initial value, is not `Send` or `Copy`, or has an annotation other than `@log` and `@label`, or one whose type has no `Display`.",
    ),
    ("E1617", "invalid-context-change", "`with Name { ... }` names something that is not a context."),
    ("E1701", "unsafe-operation-outside-region", "A raw operation outside an `unsafe` region."),
    ("E1702", "invalid-unsafe-contract", "A signature or capture does not show the `unsafe` contract it needs."),
    ("E1703", "invalid-c-type", "A type that does not cross to C in a C declaration, or an opaque type built or read."),
    (
        "E1704",
        "invalid-extern-declaration",
        "An `extern fn` or `extern type` with generics, a receiver, modes, defaults or annotations it cannot have.",
    ),
    (
        "E1705",
        "invalid-callback",
        "A value C cannot call, as a closure with captures, where an `extern fn(...)` is expected.",
    ),
    (
        "E1706",
        "duplicate-c-symbol",
        "Two functions C calls by symbol share it, or one takes `main` or a symbol starting with `tondo_`.",
    ),
    ("E1801", "script-imported-as-module", "A file with statements at its top level is imported as a module."),
    ("E1802", "duplicate-main", "The program has more than one entry point."),
    ("E1803", "invalid-main", "`main` exists, but its visibility, parameters, modifiers or result are not allowed."),
    ("E1804", "invalid-top-level-statement", "A module that is not the root has statements at its top level."),
    ("E1805", "invalid-shebang", "A `#!` line that is not the first line, or not in a root script."),
    ("E1806", "missing-main", "A program to run has no valid `main` and no root script."),
    ("E1807", "library-without-functions", "A library for C publishes no `pub extern fn` with a body."),
    ("E1901", "nonconstant-expression", "A `const` uses an operation that cannot run while compiling."),
    ("E1902", "constant-cycle", "Constants depend on each other in a cycle."),
    ("E1903", "constant-panic", "Evaluating a constant would panic."),
    ("E2001", "test-node-outside-test-source", "A `suite` or `test` in a source that is not a test source."),
    ("E2002", "duplicate-test-node", "Two suites or tests have the same identity among their siblings."),
    (
        "E2003",
        "invalid-test-source-declaration",
        "A test source exports API or is used by the program, or the program imports `std.testing`.",
    ),
    ("E2004", "empty-test-suite", "A suite has no member."),
    (
        "E2005",
        "invalid-suite-capture",
        "A member of a suite captures a `var`, a loan, an affine value, or a type that is not `Copy + Send + Share`.",
    ),
    (
        "E2006",
        "bench-outside-test-body",
        "`testing.bench` is called outside a test's own body: in a function, a closure, a `defer` or a suite's setup.",
    ),
    (
        "E2007",
        "fuzz-outside-test-body",
        "`testing.fuzz` is called outside a test's own body: in a function, a closure, a `defer` or a suite's setup.",
    ),
    (
        "E2101",
        "invalid-derive-target",
        "What `derive` names is not a nominal type of this module, or its binders differ.",
    ),
    ("E2102", "missing-derive-provider", "No provider is declared for the trait `derive` asks for."),
    (
        "E2103",
        "invalid-derive-request",
        "`derive` repeats a trait, names something that is not one, or asks what its provider does not do.",
    ),
    ("E2104", "derive-expansion-failed", "The provider rejected the declaration, with its own diagnostics."),
    (
        "E2105",
        "invalid-generated-source",
        "What a provider or generator wrote is not valid Tondo, or goes beyond what it may write.",
    ),
    (
        "E2106",
        "generator-contract-violation",
        "A generator failed, or left out, added or wrote outside its declared outputs.",
    ),
    ("E2107", "generator-resource-limit", "A provider or generator ran out of its declared steps, memory or output."),
    ("E2108", "meta-capability-denied", "Code of the `meta` profile uses an operation that profile does not have."),
    ("E2109", "generation-dependency-cycle", "Generated code depends on what the same round would generate."),
    (
        "E2110",
        "invalid-reflection",
        "A literal path or method of `std.reflect` does not exist or is not visible, or `reflect.convert` joins incompatible shapes.",
    ),
    ("W1001", "unused-import", "An import nothing uses."),
    ("W1002", "unused-binding", "A binding never read or moved."),
    ("W1003", "unused-parameter", "A named parameter a body never uses; `_` before its name says it is meant."),
    ("W1004", "naming-convention", "A name that does not follow the convention of its kind."),
    ("W1005", "confusable-identifier", "Two names in one scope that look the same."),
    ("W1006", "unreachable-code", "A statement or expression that can never run."),
    ("W1007", "redundant-conversion", "A conversion to the type the value already has."),
    ("W1008", "known-nan-comparison", "A comparison with a constant NaN, which is never true."),
    ("W1009", "avoidable-cow-copy", "A copy of a shared collection that a local loan would avoid."),
    ("W1010", "incomplete-public-documentation", "Public documentation leaves out a contract it must state."),
    ("W1011", "duplicate-set-entry", "A set literal repeats a constant entry."),
    (
        "W1012",
        "converted-field-missing",
        "`reflect.convert` fills with `none` an optional field the source does not have.",
    ),
    ("W1013", "serial-spawn-in-loop", "A `spawn` whose `Join` nobody takes, in a loop: each turn waits for its task."),
    ("P0001", "bounds", "An index or bound outside the range allowed."),
    ("P0002", "zero-slice-step", "A slice with a step of zero."),
    ("P0003", "integer-division-by-zero", "An integer division or remainder by zero."),
    ("P0004", "overlapping-borrow", "A check while running finds loans that overlap."),
    ("P0005", "checked-overflow", "Integer arithmetic beyond what its type holds."),
    ("P0006", "array-shape-mismatch", "An operation on whole arrays gets arrays of different lengths."),
    ("P0007", "assertion-failed", "`assert` got `false`."),
    ("P0008", "explicit-panic", "A call of `panic`."),
    ("P0010", "invalid-shift-count", "A shift by a negative count, or by the width of the value or more."),
    ("P0011", "invalid-repeat-count", "`Array.repeat` with a negative count."),
    ("P0012", "closed-channel", "`send` on a closed channel."),
    ("P0013", "deadlock", "Every task waits, none can go on, and nothing from C can wake them."),
    ("P0014", "reentrant-lock", "A task takes again a lock of `std.sync` it holds."),
    (
        "P0015",
        "invalid-environment-name",
        "An environment variable's name is empty or has `=` or a NUL, or its value has a NUL.",
    ),
    ("P0016", "nul-in-c-string", "A text with a zero byte inside goes to C as an `ffi.CString`."),
    ("P0017", "invalid-panic-code", "The computed `code` of `panic` is not a code of the program's own."),
    ("P0018", "negative-exponent", "`pow` of an integer, or one of its variants, gets a negative exponent."),
    ("P2001", "test-skip-during-cleanup", "`testing.skip` while a cleanup, an unwind or a teardown runs."),
    ("P2002", "test-tag-conflict", "`testing.tags` changes the value a key already has in the same test."),
    (
        "P2003",
        "test-virtual-time-deadlock",
        "Virtual time stands still: every task waits, and no timer or event can move it.",
    ),
    ("P2004", "overlapping-test-virtual-time", "A test keeps more than one domain of virtual time at once."),
    ("P2005", "test-virtual-time-range", "Virtual time moves back, or beyond what its clock holds."),
    (
        "P2006",
        "test-artifact-conflict",
        "An attachment's name or media type is invalid, or its name repeats in the same attempt.",
    ),
    ("P2007", "test-snapshot-mismatch", "A snapshot is missing or differs, outside `--update-snapshots`."),
    ("P2008", "test-snapshot-conflict", "A snapshot's name is invalid, or repeats in the same attempt."),
    (
        "P2009",
        "test-bench-conflict",
        "A benchmark's name is empty or has a control character, or repeats in the same test.",
    ),
    (
        "P2010",
        "test-fuzz-conflict",
        "A call of `testing.fuzz` has a name that is empty or has a control character, or repeats in the same test.",
    ),
    (
        "P2011",
        "test-fuzz-input-invalid",
        "An input of the fuzz store is not JSON, or does not read as the type of the call that runs it.",
    ),
];

/// What a code is: its name, its kind, what it reports, and the examples
/// of the guide that give it, each the text before it and its code.
pub fn explain(code: &str) -> Option<serde_json::Value> {
    let code = code.trim().to_uppercase();
    let &(_, name, summary) = CODES.iter().find(|(c, ..)| *c == code)?;
    let kind = match code.as_bytes()[0] {
        b'E' => "error",
        b'W' => "warning",
        _ => "panic",
    };
    let mut examples = Vec::new();
    for &(topic, text) in crate::guide::TOPICS {
        let fence = format!("```tondo error {code}");
        let mut rest = text;
        while let Some(at) = rest.find(&fence) {
            // The paragraph before the example says what to write instead.
            let before = rest[..at].trim_end();
            let said = before.rsplit("\n\n").next().unwrap_or(before).trim();
            let said = if said.starts_with("```") { "" } else { said };
            let body = &rest[at + fence.len()..];
            let body = body.strip_prefix('\n').unwrap_or(body);
            let end = body.find("```").unwrap_or(body.len());
            examples.push(serde_json::json!({ "topic": topic, "text": said, "code": &body[..end] }));
            rest = &body[end..];
        }
    }
    Some(serde_json::json!({ "code": code, "name": name, "kind": kind, "summary": summary, "examples": examples }))
}

/// `explain` for people.
pub fn text(explained: &serde_json::Value) -> String {
    let get = |key: &str| explained[key].as_str().unwrap_or_default();
    let mut out = format!("{} {} ({})\n\n{}\n", get("code"), get("name"), get("kind"), get("summary"));
    for example in explained["examples"].as_array().into_iter().flatten() {
        out.push_str(&format!("\nFrom `tondo guide {}`:\n", example["topic"].as_str().unwrap_or_default()));
        if let Some(text) = example["text"].as_str().filter(|t| !t.is_empty()) {
            out.push_str(&format!("\n{text}\n"));
        }
        out.push('\n');
        for line in example["code"].as_str().unwrap_or_default().lines() {
            out.push_str(&format!("    {line}\n"));
        }
    }
    let advice = match get("kind") {
        "panic" => {
            "\nA panic says where it happened and the stack of calls; under `tondo debug` it stops there. \
             `recover { ... }` stops it, unless it is a deadlock or the test runner's; \
             `TONDO_PANIC_FORMAT=json` writes it as JSON (`tondo guide errors`).\n"
        }
        _ => {
            "\nThe diagnostic itself says what to write; `tondo check --json` gives its repairs as data, and `tondo fix` applies the safe ones.\n"
        }
    };
    out.push_str(advice);
    out
}
