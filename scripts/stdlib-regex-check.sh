#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_REGEX_CONTRACT:-$root/testing/stdlib-regex.json}"

die() {
    echo "std.regex contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.regex"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-REGEX-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-regex.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B7"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.text"]
  and .capabilities.required == []
  and .capabilities.optional == []
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("environment")) != null)
  and ((.capabilities.forbidden | index("locale")) != null)
  and ((.capabilities.forbidden | index("dynamic-engine-registry")) != null)
  and ((.capabilities.forbidden | index("callbacks-from-pattern")) != null)
  and .unicode.version == "16.0.0"
  and .unicode.unit == "unicode-scalar"
  and .unicode.encoding == "utf-8"
  and .unicode.properties == ["General_Category", "Script", "Script_Extensions", "Binary"]
  and .unicode.case_folding == "simple-unicode-case-fold-no-multi-scalar-expansion"
  and .unicode.normalization == "none"
  and .unicode.grapheme_clusters == "not-supported"
  and .unicode.locale == "forbidden"
  and .unicode.property_unknown == "reject"
  and .syntax.dialect == "regular-only-linear"
  and .syntax.grammar == "closed"
  and .syntax.lazy_quantifiers == true
  and .syntax.greedy_default == true
  and .syntax.capture_names == "ascii-identifier-unique"
  and ((.syntax.unsupported | unique | length) == (.syntax.unsupported | length))
  and ((.syntax.unsupported | index("backreference")) != null)
  and ((.syntax.unsupported | index("lookahead")) != null)
  and ((.syntax.unsupported | index("lookbehind")) != null)
  and ((.syntax.unsupported | index("embedded-code")) != null)
  and .engine.class == "finite-automata-or-equivalent-linear-proof"
  and .engine.allowed == ["thompson-nfa", "lazy-dfa", "tagged-linear-engine"]
  and .engine.backtracking == "forbidden"
  and .engine.host_recursion == "forbidden"
  and .engine.worklists == "explicit-bounded"
  and .engine.matching_complexity == "linear-in-input-within-max-steps"
  and .engine.deterministic == true
  and .semantics.search == "leftmost"
  and .semantics.tie_break == "greedy-longest-then-alternative-order"
  and .semantics.find_all == "non-overlapping"
  and .semantics.zero_width_progress == "one-unicode-scalar"
  and .semantics.offsets == "utf8-byte-half-open-scalar-boundaries"
  and .replacement.tokens == ["dollar-zero", "dollar-number", "braced-name", "double-dollar"]
  and .replacement.unknown_capture == "reject"
  and .replacement.callback == "not-supported"
  and .replacement.atomic_output == true
  and .api.module == "std.regex"
  and ([.api.functions[]] | sort) == ["compile", "findAll", "isFullMatch", "isMatch", "match", "replace", "replaceAll"]
  and .api.iterator_methods == ["next"]
  and .api.terminal_state == "iterator-none-is-terminal"
  and .api.unicode_descriptor == "recorded-at-compile"
  and ([.surface.types[]] | sort) == ["Regex", "RegexCapture", "RegexError", "RegexErrorKind", "RegexFindIterator", "RegexLimits", "RegexMatch", "RegexOptions", "RegexSpan"]
  and (.surface.signatures | length) == 16
  and ([.surface.signatures[].id] | unique | length) == 16
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and .effect == "pure")
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | length) == 0
  and .surface.direct_call_waits == false
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.selectable_operations == []
  and .surface.no_async_duplicate_api == true
  and .ownership.regex_immutable_shareable == true
  and .ownership.iterator_borrows_input == true
  and .ownership.iterator_outlives_input == false
  and .ownership.captures_are_spans == true
  and .ownership.span_slice_copies == true
  and .ownership.callback_storage == false
  and ([.limits[].id] | sort) == ["max_capture_groups", "max_class_ranges", "max_input_bytes", "max_matches", "max_output_bytes", "max_pattern_bytes", "max_program_states", "max_repeat", "max_replacement_bytes", "max_steps", "max_syntax_depth", "vm_heap"]
  and .errors.type == "RegexError"
  and .errors.location == "phase-and-pattern-or-input-byte-offset"
  and .errors.partial_success == false
  and (.errors.kinds | length) == 22
  and ((.errors.kinds | unique | length) == (.errors.kinds | length))
  and ((.errors.kinds | index("UnsupportedFeature")) != null)
  and ((.errors.kinds | index("StepLimitExceeded")) != null)
  and ((.errors.kinds | index("InvalidReplacement")) != null)
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.matching == "linear-with-explicit-step-budget"
  and .performance.parser_stack == "explicit-worklist"
  and .performance.claims_before_perf_gate == "forbidden"
  and ([.test_matrix[].id] | unique | length) == 10
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | length) == 8
  and ([.corpora[].id] | unique | length) == 8
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-applicable-pure-core"
  and .implementation.required_follow_ups == ["STD-REGEX-IMPL-001", "STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.regex contract"

for path in \
    docs/contracts/stdlib-regex.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-REGEX-001' \
    'Unicode 16.0.0' \
    'RegexOptions.caseInsensitive' \
    'RegexFindIterator' \
    'RegexSpan.slice' \
    'max_program_states' \
    'StepLimitExceeded' \
    'UnsupportedFeature' \
    'backtracking exponencial' \
    'look-ahead' \
    'replacement callbacks' \
    'worklists explícitos' \
    'No hay una API async'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-regex.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-regex.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the regex registry"

echo "std.regex contract: OK (Unicode 16.0.0; finite automata; captures; bounded replace)"
