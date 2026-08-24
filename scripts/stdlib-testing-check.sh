#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_STDLIB_TESTING_CONTRACT:-$root/testing/stdlib-testing.json}"

if [[ ! -f "$contract" ]]; then
    echo "missing std.testing owner contract: ${contract#"$root"/}" >&2
    exit 1
fi

if ! tail -c 1 "$contract" | cmp -s <(printf '\n'); then
    echo "std.testing owner contract must end with one LF" >&2
    exit 1
fi

if grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null; then
    echo "std.testing owner contract contains CR or trailing whitespace" >&2
    exit 1
fi

jq -e '
    def unique_values: length == (unique | length);

    .format == "tondo-stdlib-owner-contract/1"
    and .owner == "std.testing"
    and .edition == "0.1"
    and .phase == "STD-0.1A"
    and .status == "closed-contract"
    and .base.normative_document == "TONDO_TESTING_SPEC.md"
    and .base.sealed_core == [
      "log", "tags", "failNow", "skip", "attach", "snapshot",
      "withVirtualTime", "VirtualTime.settle", "VirtualTime.advance"
    ]
    and .base.core_unchanged == true
    and .base.test_only == true
    and .base.production_import == false
    and .base.runtime_registration == false
    and .base.runtime_reflection == false
    and .base.panic_recovery == false
    and .base.tag_selector == false
    and .base.lifecycle_hooks == false
    and .capabilities.core == []
    and .capabilities.temporary == ["filesystem"]
    and .capabilities.forbidden == [
      "console", "environment", "clock", "civil-clock", "entropy", "network", "process", "threads"
    ]
    and .assertions.failure == "P0007"
    and .assertions.value_observation == "ref-no-move"
    and .assertions.display == "static-Display-no-reflection"
    and (.assertions.functions | map(.id)) == ["assertEqual", "assertNotEqual", "assertTextEqual"]
    and all(.assertions.functions[]; (.capabilities | length) == 0 and (.format | length) > 0)
    and .assertions.functions[0].signature == "[T: Equatable + Display](expected: ref T, actual: ref T): Unit"
    and .assertions.functions[0].ownership == "borrow-both"
    and .assertions.functions[1].signature == "[T: Equatable + Display](expected: ref T, actual: ref T): Unit"
    and .assertions.functions[1].ownership == "borrow-both"
    and .assertions.functions[2].signature == "(expected: String, actual: String): Unit"
    and .assertions.functions[2].ownership == "copy-string"
    and (.text_diff.hunk_kinds | unique_values)
    and .text_diff.hunk_kinds == ["Equal", "Delete", "Insert"]
    and .text_diff.type == "TextDiff"
    and .text_diff.hunk_type == "TextDiffHunk"
    and .text_diff.function == "diffText"
    and .text_diff.render_method == "TextDiff.render"
    and .text_diff.format == "tondo-test-text-diff-0.1/1"
    and .text_diff.algorithm == "myers-shortest-edit-script-v1"
    and .text_diff.granularity == "lines-preserve-lf-and-cr-bytes"
    and .text_diff.tie_break == "expected-byte-offset-then-actual-byte-offset"
    and .text_diff.normalization == "none"
    and .text_diff.adjacent_hunks == "coalesce-same-kind"
    and .text_diff.equal_result == "empty-hunks"
    and .text_diff.truncation == "flag-prefix-no-partial-hunk"
    and .text_diff.snapshot_store == false
    and .text_diff.snapshot_update == false
    and .text_diff.capabilities == []
    and .text_diff.fields == ["equal", "hunks", "expectedBytes", "actualBytes", "truncated"]
    and .floats.tolerance_type == "FloatTolerance"
    and .floats.tolerance_opaque == true
    and .floats.constructor == "FloatTolerance.from"
    and .floats.fields == ["absolute", "relative"]
    and .floats.domain == "finite-non-negative"
    and .floats.formula == "abs(actual-expected) <= max(absolute, relative*max(abs(expected),abs(actual)))"
    and .floats.nan == "always-fails"
    and .floats.infinity == "only-identical-sign-passes"
    and .floats.signed_zero == "equal"
    and .floats.float32 == "exact-widen-to-Float"
    and .floats.assertions == ["assertFloatNear", "assertFloat32Near"]
    and .floats.failure == "P0007"
    and .floats.errors == ["Negative", "NonFinite", "Overflow"]
    and .floats.capabilities == []
    and (.option_result.functions | map(.id)) == ["assertSome", "assertNone", "assertOk", "assertErr"]
    and .option_result.functions[0].signature == "[T](value: T?): T"
    and .option_result.functions[1].signature == "[T](value: T?): Unit"
    and .option_result.functions[2].signature == "[T, E: Display](value: T ! E): T"
    and .option_result.functions[3].signature == "[T: Display, E](value: T ! E): E"
    and all(.option_result.functions[]; .consumes == (if (.id | startswith("assertSome") or startswith("assertNone")) then "Option" else "Result" end) and .mismatch == "P0007")
    and .option_result.implicit_propagation == false
    and .option_result.wrapper_reordering == false
    and .option_result.ownership == "consume-wrapper-return-payload"
    and .temporary.type == "TempDirectory"
    and .temporary.error_type == "TempError"
    and .temporary.create == "tempDirectory(prefix: String): TempDirectory ! TempError"
    and .temporary.path == "TempDirectory.path(ref self): Path"
    and .temporary.cleanup == "TempDirectory.cleanup(self): Unit"
    and .temporary.capabilities == ["filesystem"]
    and .temporary.ownership == "affine-terminal"
    and .temporary.send == true
    and .temporary.share == false
    and .temporary.prefix_alphabet == "ASCII-A-Za-z0-9._-"
    and .temporary.prefix_empty_allowed == true
    and .temporary.prefix_max_bytes == 32
    and .temporary.root == "worker-private-runner-root"
    and .temporary.host_nonce == true
    and .temporary.ambient_temp_variables == false
    and .temporary.report_path == false
    and .temporary.cleanup_strategy == "recursive-root-bounded-no-symlinks"
    and .temporary.cleanup_failure == "infrastructure-terminal"
    and .temporary.forced_termination_cleanup == "runner-owned"
    and .temporary.error_values == ["InvalidPrefix", "Unavailable", "PermissionDenied", "LimitExceeded", "IoError"]
    and .temporary.duplicate_file_api == false
    and .generation.types == ["Generator", "GenerationId"]
    and .generation.constructor == "Generator.new(seed: UInt64)"
    and .generation.replay_constructor == "Generator.forCase(seed: UInt64, caseIndex: UInt64)"
    and .generation.id_fields == ["seed", "caseIndex"]
    and .generation.case_index == "zero-based-UInt64"
    and .generation.algorithm == "xorshift64-7-9-8-v1"
    and .generation.state_initialization == "seed-xor-0x9e3779b97f4a7c15"
    and .generation.case_derivation == "seed-plus-caseIndex-times-0x9e3779b97f4a7c15-mod-UInt64"
    and .generation.zero_state == "0x6a09e667f3bcc909"
    and .generation.draw_sequence == ["shift-left-7", "shift-right-9", "shift-left-8"]
    and .generation.draws == ["nextUInt", "nextBool", "nextInt", "nextBytes", "nextText"]
    and .generation.int_range == "inclusive-unbiased-rejection"
    and .generation.bytes_length == "uniform-zero-through-maximum"
    and .generation.text_encoding == "valid-unicode-scalars-canonical-utf8"
    and .generation.shrink_trait == "Shrink"
    and .generation.shrink_function == "shrink(ref value)"
    and .generation.shrink_protocol == "compiler-sealed-intrinsic"
    and .generation.custom_shrink_implementations == false
    and .generation.custom_shrink_rejection == "E1114-closed-protocol"
    and .generation.shrink_builtins == ["integers", "floats", "String", "Array[T]"]
    and .generation.shrink_order == "lowest-complexity-first"
    and .generation.shrink_duplicates == "remove-preserve-first"
    and .generation.shrink_executes_predicate == false
    and .generation.panic_recovery == false
    and .generation.test_registration == false
    and .generation.security == "deterministic-non-cryptographic-no-secrets"
    and .generation.format == "tondo-test-generation-0.1/1"
    and .generation.capabilities == []
    and .generation.runner.module == "crates/tondo-compiler/src/test_generation.rs"
    and .generation.runner.runtime == "RuntimeRunner"
    and .generation.runner.case_order == "generation-index"
    and .generation.runner.replay == "Generator.forCase(seed, caseIndex)"
    and .generation.runner.fresh_worker_per_case == true
    and .generation.runner.fresh_worker_per_shrink_candidate == true
    and .generation.runner.dynamic_test_registration == false
    and .generation.runner.report_format_unchanged == true
    and .generation.runner.max_cases == 100000
    and .generation.runner.max_shrink_candidates == 4096
    and .generation.runner.max_shrink_depth == 64
    and (.limits | map(.id)) == [
      "max_assertion_message_bytes", "max_display_bytes", "max_diff_input_bytes",
      "max_diff_lines", "max_diff_hunks", "max_diff_output_bytes",
      "max_temp_prefix_bytes", "max_temp_tree_entries", "max_temp_bytes",
      "max_generator_draws", "max_generated_bytes", "max_shrink_candidates",
      "max_shrink_depth"
    ]
    and all(.limits[]; (.unit | length) > 0 and (.scope | length) > 0 and (.check | length) > 0)
    and .errors.float_tolerance == ["Negative", "NonFinite", "Overflow"]
    and .errors.temporary == ["InvalidPrefix", "Unavailable", "PermissionDenied", "LimitExceeded", "IoError"]
    and .errors.generation == ["InvalidRange", "InvalidLength", "LimitExceeded", "OutputTooLarge"]
    and .formats.assertion == "tondo-test-assertion-0.1/1"
    and .formats.text_diff == "tondo-test-text-diff-0.1/1"
    and .formats.generation == "tondo-test-generation-0.1/1"
    and .formats.report_reuse == ["tondo-test-report-0.1/7", "tondo-junit-report-0.1/4"]
    and .formats.new_snapshot_store == false
    and .formats.new_junit_schema == false
    and (.corpora | map(.id)) == [
      "assertions-and-display", "text-diff", "float-tolerance",
      "option-result-consumption", "temporary-resources", "generation-replay",
      "generation-shrink", "capability-and-boundary"
    ]
    and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0 and (.focus | unique_values))
    and (.test_matrix | map(.id)) == [
      "assertions", "text-diff", "float-tolerance", "option-result", "temporary",
      "generation", "shrinking", "core-compatibility"
    ]
    and all(.test_matrix[]; .required == true and (.observables | length) > 0 and (.observables | unique_values))
    and (.promotion.gates | map(.id)) == ["design", "implementation", "conformance", "security", "promote"]
    and .promotion.gates[0].requires == ["sealed-core-boundary", "exact-signatures", "ownership", "capabilities", "finite-limits"]
    and .promotion.gates[1].requires == ["typed-assertions", "bounded-diff", "float-policy", "option-result-consumption", "temp-cleanup", "seeded-generator"]
    and .promotion.gates[2].requires == ["owner-corpus", "cross-backend", "format-stability", "capability-rejection", "failure-precedence"]
    and .promotion.gates[3].requires == ["no-host-entropy", "no-secret-generator", "root-bounded-cleanup", "no-reflection", "no-panic-capture"]
    and .promotion.gates[4].requires == ["all-required-matrix", "report-and-snapshot-compatibility", "coverage-baseline", "STD-PERF-001-report"]
    and .promotion.next_coordination == "STD-CONF-001"
' "$contract" >/dev/null

echo "std.testing owner contract: OK"
