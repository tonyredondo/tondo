#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_YAML_CONTRACT:-$root/testing/stdlib-yaml.json}"

die() {
    echo "std.yaml contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.yaml"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-YAML-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-yaml.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B7"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.serialization", "std.io", "std.bytes", "std.encoding"]
  and .capabilities.required == []
  and .capabilities.optional == []
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("environment")) != null)
  and ((.capabilities.forbidden | index("external-includes")) != null)
  and ((.capabilities.forbidden | index("code-execution")) != null)
  and .wire.yaml_version == "1.2"
  and .wire.schema == "core"
  and .wire.encoding == "UTF-8"
  and .wire.indentation == "spaces-only"
  and .wire.document_markers == ["---", "..."]
  and .wire.block_collections == true
  and .wire.flow_collections == true
  and .wire.quoted_scalars == ["single", "double"]
  and .wire.block_scalars == ["literal", "folded"]
  and .wire.directives == "YAML-1.2-only-no-TAG"
  and .wire.mapping_keys == "text-only"
  and .wire.duplicate_keys == "reject"
  and .wire.merge_key == "reject"
  and .wire.implicit_timestamp == "text"
  and .wire.yaml_1_1_booleans == "text"
  and .wire.non_finite_numbers == "reject"
  and .tags.accepted_short == ["!!null", "!!bool", "!!int", "!!float", "!!str", "!!binary", "!!seq", "!!map"]
  and .tags.accepted_full_prefix == "tag:yaml.org,2002:"
  and .tags.custom == "reject"
  and .tags.local == "reject"
  and .tags.timestamp == "reject"
  and .tags.binary == "standard-base64-required-padding"
  and .tags.compatibility == "tag-must-match-node"
  and .aliases.accepted == true
  and .aliases.scope == "single-document"
  and .aliases.forward_reference == "reject"
  and .aliases.redefinition == "reject"
  and .aliases.cycles == "reject"
  and .aliases.dynamic_model == "logical-copy"
  and .aliases.preserve_identity == false
  and .aliases.merge_key == "reject"
  and .aliases.max_aliases_source == "YamlLimits.maxAliases"
  and .aliases.max_expanded_nodes_source == "YamlLimits.maxExpandedNodes"
  and .aliases.anchor_name_limit_source == "YamlLimits.maxAnchorNameBytes"
  and .surface.types == ["Yaml", "YamlValue", "YamlValueView", "YamlTag", "YamlScalar", "YamlEvent", "YamlLimits", "YamlOptions", "YamlPathSegment", "YamlErrorKind", "YamlError", "YamlReader", "YamlWriter"]
  and (.surface.signatures | length) == 21
  and ([.surface.signatures[].id] | unique | length) == 21
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | sort) == ["reader-from-reader", "writer-finish", "writer-to-writer", "writer-write"]
  and .surface.direct_call_waits == false
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "unchanged-by-language"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == []
  and .surface.no_async_duplicate_api == true
  and .dynamic_model.object == "ordered-map-string-keyed"
  and .dynamic_model.array == "ordered-values"
  and .dynamic_model.binary == "YamlValue.Bytes-only-from-explicit-binary-tag"
  and .dynamic_model.alias == "expanded-logical-copy"
  and .dynamic_model.shared_identity == false
  and .dynamic_model.non_string_keys == "reject"
  and .dynamic_model.view_lifetime == "until-reader-advance-or-operation-return"
  and .scalar_rules.null == ["null", "~", "empty-mapping-value"]
  and .scalar_rules.bool == ["true", "false", "ASCII-case-insensitive"]
  and .scalar_rules.integer == ["decimal", "0b", "0o", "0x"]
  and .scalar_rules.float == ["finite-decimal", "scientific"]
  and .scalar_rules.timestamp == "text-unless-explicit-rejected-tag"
  and .scalar_rules.yaml_1_1_boolean_spellings == "text"
  and .scalar_rules.non_finite == "reject"
  and .scalar_rules.integer_overflow == "reject"
  and .scalar_rules.implicit_locale == false
  and .streaming.materialized_is_collector == true
  and .streaming.chunk_boundary_invariant == true
  and .streaming.event_model == "explicit-stream-document-collection-anchor-alias-frames"
  and .streaming.reader_input == "std.io.Reader-until-StreamEnd"
  and .streaming.writer_output == "std.io.Writer-via-writeAll"
  and .streaming.empty_chunk == "no-state-change"
  and .streaming.finish_required == true
  and .streaming.error_state == "terminal"
  and .streaming.post_finish == "YamlError.Closed"
  and .streaming.resource_limit_write == "atomic-no-state-change"
  and .streaming.partial_tondo_result == "never-published"
  and .streaming.stack == "explicit-bounded-frames"
  and .ownership.value_owner == "YamlValue"
  and .ownership.options_copy == true
  and .ownership.reader_writer_affine == true
  and .ownership.reader_writer_copy == false
  and .ownership.reader_writer_share == false
  and .ownership.reader_writer_send == true
  and .ownership.view_borrow == "ends-at-reader-advance-or-operation-return"
  and .ownership.alias_storage == "logical-copy-no-shared-mutable-memory"
  and .ownership.input_alias == "never-retained"
  and ([.limits[].id] | sort) == ["max_aliases", "max_anchor_name_bytes", "max_collection_entries", "max_depth", "max_documents", "max_expanded_nodes", "max_input_bytes", "max_nodes", "max_scalar_bytes", "vm_heap"]
  and .errors.type == "YamlError"
  and .errors.location == "UTF-8-byte-offset-plus-one-based-line-column"
  and .errors.path == "stable-Key-or-Index-array"
  and .errors.partial_success == false
  and .errors.alias_failure_before_expansion == true
  and (.errors.kinds | length) == 32
  and ((.errors.kinds | unique | length) == (.errors.kinds | length))
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.dispatch == "target-declared-and-chunk-size-based"
  and .performance.parser_stack == "explicit-worklist"
  and .performance.alias_expansion == "iterative-and-budgeted"
  and .performance.streaming_allocation == "bounded-by-chunk-and-limits"
  and .performance.claims_before_perf_gate == "forbidden"
  and ([.test_matrix[].id] | unique | length) == 8
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and (([.corpora[].id] | unique | length) == ([.corpora[].id] | length))
  and ([.corpora[].id] | unique) == ["anchors-and-adversarial-aliases", "fragmentation-and-errors", "invalid-and-security", "yaml-1.2-core"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-YAML-IMPL-001", "STD-YAML-TEST-001", "STD-YAML-PERF-001", "STD-YAML-CONF-001", "STD-YAML-DOC-001"]
  and .promotion.next_blocks == ["STD-LOG-001", "DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.yaml contract"

for path in \
    docs/contracts/stdlib-yaml.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-YAML-001' \
    'pub type YamlValue' \
    'pub enum YamlEvent' \
    'pub type YamlLimits' \
    'pub fn parseAll' \
    'pub fn encodeCanonical' \
    'YamlReader.fromReader' \
    'YamlWriter.toWriter' \
    'YamlLimits.maxExpandedNodes' \
    'copia lógica' \
    'MergeKeyForbidden' \
    'YAML 1.2 Core' \
    'YamlReader.next' \
    'YamlWriter.finish' \
    'UTF-8' \
    'frames explícitos' \
    'NoProgress'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-yaml.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-yaml.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the YAML registry"

echo "std.yaml contract: OK (YAML 1.2 core; bounded aliases; typed/dynamic/streaming; no ambient resolution)"
