# Diagnostics JSON 0.1

**Status:** frozen public format for Tondo 0.1

This contract defines the machine-readable diagnostics emitted by the Tondo
0.1 CLI. It is independent of the human renderer.

## Transport

Diagnostics use UTF-8 JSON Lines on stderr:

- each diagnostic is one compact JSON object followed by one LF byte;
- an empty report emits no bytes;
- stdout remains available for formatter or program output; and
- ANSI escapes never appear in JSON output.

Every object contains exactly these keys, serialized in this order:

~~~text
id, severity, code, message, source_id, module, file, range,
expected, actual, related, fixes
~~~

`module`, `file`, `range`, `expected`, and `actual` are present even when their
value is `null`. `related` and `fixes` are always arrays.

## Primary diagnostic

~~~json
{
  "id": "diag:<64 lowercase hexadecimal digits>",
  "severity": "error",
  "code": "E1102",
  "message": "expected Int, found String",
  "source_id": "root:cli",
  "module": "main",
  "file": "main.to",
  "range": {
    "start": {"byte": 20, "line": 0, "column": 20},
    "end": {"byte": 27, "line": 0, "column": 27}
  },
  "expected": "Int",
  "actual": "String",
  "related": [],
  "fixes": []
}
~~~

The closed severities are `error` and `warning`. A diagnostic code is one
uppercase ASCII letter followed by four decimal digits. `message` is non-empty
and contains no LF. Exact wording is not a stable API unless a conformance case
explicitly pins it; code, severity, identity, locations, values, children, and
ordering are stable.

Ranges are half-open. `byte` is a zero-based UTF-8 byte offset. `line` and
`column` are zero-based; columns count Unicode scalar values, not bytes or
grapheme clusters. If a position lies after the valid UTF-8 prefix of an
invalid source, its object contains only `byte`. A target-level diagnostic has
`module`, `file`, and `range` set to `null`.

For edition `0.1`, the stable diagnostic ID is:

~~~text
diag:sha256(
  "0.1\n"
  + source_id + "\n"
  + (module or "") + "\n"
  + (file or "") + "\n"
  + code + "\n"
  + (start byte or "") + "\n"
  + (end byte or "") + "\n"
)
~~~

The digest is encoded as 64 lowercase hexadecimal digits. Message text,
severity, expected/actual values, related locations, and fixes do not
participate in the ID. Diagnostics that resolve to the same ID are merged.

## Related locations and fixes

A related location contains exactly:

~~~text
message, source_id, module, file, range
~~~

It always names a source range. A fix contains exactly:

~~~text
title, applicability, edits
~~~

`applicability` is `safe` or `requires-decision`. Each edit contains exactly:

~~~text
source_id, module, file, range, replacement
~~~

Fix titles are non-empty. A fix contains at least one edit, and edits in one
fix cannot overlap within a file. `replacement` is an arbitrary UTF-8 string
and may contain line feeds.

The Tondo 0.1 conformance runner does not trust a `safe` label by shape alone.
It applies all edits to the exact request snapshot in descending byte order per
file, reruns the check, and accepts the fix only when the result succeeds
without errors.

## Normative ordering

Diagnostics are sorted by:

1. `source_id`;
2. optional `module`;
3. optional `file`;
4. optional start byte;
5. optional end byte;
6. severity;
7. code; and
8. message.

Optional values sort before present values. Severity order is `error`, then
`warning`. Related locations sort by source identity, module, file, range, and
message. Edits sort by their complete serialized content. Fixes sort by
applicability, title, and edits. Exact duplicates are removed.

This ordering and the schema above are part of
`tondo-conformance-0.1`. A later incompatible public format must use a new
explicit version; it cannot silently change the 0.1 stream.
