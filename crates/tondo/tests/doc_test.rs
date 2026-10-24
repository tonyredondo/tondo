//! `tondo doc-test` (21.6) on documents with an example of each kind.

use std::path::{Path, PathBuf};
use std::process::Command;

fn document(name: &str, text: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("doc-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// Runs `tondo doc-test`: stdout, stderr and the exit status.
fn doc_test(path: &Path) -> (String, String, i32) {
    let output =
        Command::new(env!("CARGO_BIN_EXE_tondo")).args(["doc-test", "--edition", "0.1"]).arg(path).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

const EXAMPLES: &str = "\
# Examples

~~~tondo
let total = price * count
~~~

~~~tondo
fn area(width: Int, height: Int): Int
~~~

~~~tondo fragment spec.user
fn label(user: User): String {
    \"{user.name} ({user.id.value})\"
}
~~~

~~~tondo fragment spec.resource
fn reserve(): !AcquireError {
    let resource = acquire()?
    Resource.release(resource)
}
~~~

~~~tondo compile-fail E1401
fn invalid(resource: Resource) {
    let pending = resource
    Resource.release(resource)
    Resource.release(pending)
}
~~~

~~~tondo pseudocode
anything at all
~~~

```tondo
not an example of the document: its fences are `~~~`
```
";

#[test]
fn checks_each_kind_of_example() {
    let (stdout, stderr, status) = doc_test(&document("examples.md", EXAMPLES));
    assert_eq!(status, 0, "{stderr}");
    let records: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let records = records.as_array().unwrap();
    let field = |key: &str| records.iter().map(|r| r[key].clone()).collect::<Vec<_>>();
    let categories = ["syntax", "syntax", "fragment", "fragment", "compile-fail", "pseudocode"];
    assert_eq!(field("category"), categories.map(serde_json::Value::from));
    assert_eq!(records[0]["production"], serde_json::json!(["syntax_sequence", "statement"]));
    assert_eq!(records[1]["production"], serde_json::json!(["syntax_sequence", "function_signature"]));
    assert_eq!(records[2]["production"], serde_json::json!(["script_program", "fragment_wrapper"]));
    assert_eq!(field("typecheck_ok")[2..5], serde_json::json!([true, true, false]).as_array().unwrap()[..]);
    assert_eq!(records[4]["expected_codes"], serde_json::json!(["E1401"]));
    assert_eq!(records[4]["actual_codes"], serde_json::json!(["E1401"]));
    assert_eq!(records[5]["parse_ok"], serde_json::Value::Null);
    assert_eq!(records[2]["fixture"], "spec.user");
    assert_eq!(records[0]["fixture"], serde_json::Value::Null);
    // The source of an example ends with one line feed.
    let source = "let total = price * count\n";
    assert_eq!(records[0]["source_sha256"], tondo::hash::sha256_hex(source.as_bytes()));
}

#[test]
fn reports_failing_examples() {
    let text = "\
~~~tondo fragment
fn twice(value: Int): Int {
    value * \"2\"
}
~~~

~~~tondo compile-fail E1003
let value = 1
~~~
";
    let (stdout, stderr, status) = doc_test(&document("failing.md", text));
    assert_eq!(status, 1);
    assert!(stdout.starts_with('['), "{stdout}");
    assert!(stderr.contains("failing.md:1: this example does not compile"), "{stderr}");
    assert!(stderr.contains("failing.md:7: this example should fail with E1003, and it compiles"), "{stderr}");
}

#[test]
fn rejects_documents_it_cannot_read() {
    let cases = [
        ("~~~tondo example\nlet x = 1\n~~~\n", "error[E0006]: unknown header `tondo example`"),
        ("~~~tondo compile-fail E9999\nlet x = 1\n~~~\n", "`E9999` is not an error code of the registry"),
        ("~~~tondo fragment spec.nowhere\nlet x = 1\n~~~\n", "there is no fixture `spec.nowhere`"),
        ("~~~tondo\nlet x = 1\n", "this example has no closing `~~~` line"),
        ("text\rmore\n", "error[E0002]"),
    ];
    for (text, expected) in cases {
        let (stdout, stderr, status) = doc_test(&document("broken.md", text));
        assert_eq!(status, 1);
        assert!(stdout.is_empty() && stderr.contains(expected), "{text:?}: {stderr}");
    }
}

/// The examples of the specifications hold.
#[test]
fn the_specifications_examples_hold() {
    let specs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec");
    let names = [
        "TONDO_LANGUAGE_SPEC.md",
        "TONDO_STANDARD_LIBRARY_SPEC.md",
        "TONDO_TESTING_SPEC.md",
        "TONDO_TOOLCHAIN_SPEC.md",
    ];
    for name in names {
        let (_, stderr, status) = doc_test(&specs.join(name));
        assert_eq!(status, 0, "{name}:\n{stderr}");
    }
}
