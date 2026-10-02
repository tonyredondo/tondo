# std.cbor bounded testing contract

`STD-CBOR-TEST-001` verifies the independent wire model, retained regression
corpus and bounded fuzz boundary of the Rust scalar kernel. The register is
[testing/stdlib-cbor-test.json](../../testing/stdlib-cbor-test.json); the owner
contract remains [stdlib-cbor.md](./stdlib-cbor.md).

## Independent oracle and domain

`crates/tondo-reliability/src/cbor_model.rs` implements a recursive RFC 8949
grammar independently of the production parser's explicit worklists. It imports
no production codec. The oracle preserves tags, ordered arbitrary-key pairs,
integer magnitudes, float wire bits, undefined and unassigned simple values.
Its ordinary renderer emits definite lengths; its deterministic renderer sorts
encoded key bytes, rejects collisions, preserves zero sign, chooses the
shortest exact float width and emits every NaN as `f97e00`. Binary16 conversion
uses arithmetic over exact powers of two, rather than production bit fields.

The domain is deliberately finite: 128 value nodes, eight levels, 96 payload
bytes per scalar, 128 chunks per indefinite string, 4096 input/output bytes and
512 replay steps. `Limit` means outside this reference domain; it is not a
claim that the production input is malformed. Each indefinite text chunk must
independently contain valid UTF-8. Malformed framing and trailing items are
rejected before returning a value. No clock, path, environment or registry
affects model output.

## Executable evidence

The persistent corpus at
`crates/tondo-reliability/tests/fixtures/cbor-wire.json` contains 61 valid and
30 invalid RFC wire and owner regression vectors. It is a quality provenance
input under `crates`, and is embedded by the integration suite. Its expected
ordinary and deterministic bytes are checked against both the reference and
production encoders. The suite also covers exact raw preservation, borrowed
encoded-byte views, owned event replay, policies, typed collections, malformed
input, half-open byte spans, nested paths, resource limits, fragmentation,
writer balance, I/O errors and terminal states.

Nine integration tests compare both encoders for 4096 deterministic seeds and
exercise all 65536 binary16 patterns, including NaNs and signed zero. Each half
value is also widened through the independent arithmetic oracle and tested as
binary32 and binary64. These observations complement the 14 kernel unit tests;
the independent model does not claim complete validation of every RFC input.

The test block fixes a confirmed RFC 8949 section 3.3 defect: `f8` followed by
any value below 32 is malformed, even under the permissive non-minimal policy.
All 32 encodings are rejected through parse, validate, raw, view, event reader
and typed decode. Permissive decoding still accepts well-formed overlong
integer and length arguments; those are different wire rules.

## Reproducible bounded fuzz

`scripts/stdlib-cbor-fuzz.sh` materializes exactly the retained hex vectors and
the seed at `fuzz/corpus/stdlib_cbor/seed` into a fresh task artifact directory.
It retains generated outputs separately. The smoke uses `nightly-2026-07-28`,
128 runs, seed 4113, 4096-byte inputs, a ten-second per-input timeout and a
4096 MiB RSS limit. Sanitizer settings are not weakened. Empty input is covered
by integration tests; libFuzzer ignores empty corpus files.

The standalone target includes the canonical model directly and disables the
fuzz workspace's default `compiler-targets` feature. It compares two reference
replays, all generated values' ordinary/deterministic bytes and production
parse results, then compares arbitrary input acceptance within the reference
domain. Inputs outside that domain carry no interoperability claim. The focused
script verifies Cargo's resolved dependency graph excludes compiler, VM,
conformance and reliability CLI packages, and builds all ordinary fuzz targets.

Run `scripts/stdlib-cbor-test-test.sh` for the positive and negative contract
checks and focused Rust suites; run `scripts/stdlib-cbor-fuzz.sh` for the smoke.
`CARGO_TARGET_DIR` selects the task's artifact disk.

The contract checker uses Bash, jq and standard grep/coreutils; ripgrep is not
a runner dependency. The focused script checks a restricted `PATH` without
ripgrep and rejects a missing inspection utility, so an unavailable tool cannot
be interpreted as proof that the model has no production imports.

## Promotion boundary

Only the independent bounded model, Rust kernel tests, corpus and bounded fuzz
are promoted. This block does not register a public compiler API or production
VM host, implement a native ABI or native AOT lowering, or establish SIMD,
code-size or performance claims. The kernel remains
`verified-stdlib-kernel`, with `public_api_promoted: false` and
`host: not-claimed-until-compiler-cbor-abi`. This test leaf records performance
as its historical unlock, followed by conformance and usage documentation.
The parent owner register carries the current next block as later leaves close.
At test-leaf closure the full functional gate and
provenance-bound quality checks pass. Global line coverage is
91.5653%; every global and risk-scope line/function/region dimension meets the
unchanged 80% floor, and all six selected critical mutants are caught. This
evidence does not change the pending S1A prerequisites or release status.
