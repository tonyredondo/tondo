# Test harness contract

**Status:** accepted for bootstrap

## Fixture classes

The repository reserves these roots:

~~~text
tests/spec/
tests/compile-pass/
tests/compile-fail/
tests/runtime/
~~~

Cases are discovered in lexicographic logical-path order. A `.to` file is the
source. Optional adjacent files use the same basename:

- `.codes`: one expected diagnostic code per line; mandatory for spec and
  compile-fail fixtures.
- `.jsonl`: exact structured diagnostic snapshot.
- `.stderr`: exact human diagnostic.
- `.stdout`: exact formatter or program stdout.
- `.runtime-stderr`: exact program stderr.
- `.exit`: decimal process exit code.

No test may infer success from a missing expected file. Each fixture class has a
closed default contract documented by its directory.

## Inline fixtures

Unit and integration tests may construct `SourceInput::virtual_file` directly.
They must still provide source ID, module, logical path, edition, target, profile,
capabilities, and limits through the normal driver.

Inline tests are preferred for small algorithmic behavior. Filesystem fixtures
are preferred for stable public output and multi-file behavior.

## Specification fence extraction

The maintained test extractor follows language-spec section 21.6 directly; it
does not delegate scanning to a Markdown library. For the pinned `0.1` edition
it:

- Recognizes only an unindented opening line beginning exactly with `~~~tondo`
  and an exact unindented `~~~` closer.
- Accepts only `tondo`, `fragment`, `script`, `compile-fail`, and `pseudocode`
  header forms defined by the spec.
- Normalizes fence content to LF and appends one final LF.
- Records the opening byte offset and processes fences in that order.
- Carries the explicit fixture name or the normative `spec.0_1` default.
- Carries the exact distinct `Edddd` set for `compile-fail`.
- Runs every non-pseudocode fence through the ordinary edition, target,
  formatter, diagnostic, and fixture paths.
- Emits the complete machine-readable result record and SHA-256 fields required
  by section 21.6.

Extraction failures are document failures, not Tondo source diagnostics. Every
lexically valid, syntactically valid non-pseudocode fence is also formatted,
reparsed, and checked for a second identical formatting result.

## Regression rule

Every compiler bug receives the smallest Tondo input that reproduces it and a
test at the lowest public boundary that would have caught it. A test must not
depend on physical absolute paths, wall time, locale, network, hash iteration,
or scheduling order.

## Conformance separation

Implementation fixtures may test private invariants and `T` diagnostics. The
future `tondo-conformance-0.1` suite contains only normative behavior and can run
against another implementation through adapters.
