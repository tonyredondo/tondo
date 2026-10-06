#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_REGEX_CONTRACT:-testing/stdlib-regex.json}"
document="${TONDO_STDLIB_REGEX_DOCUMENT:-docs/contracts/stdlib-regex.md}"
example="crates/tondo-stdlib/examples/regex_usage.rs"
die() { echo "std.regex documentation: $*" >&2; exit 1; }
for path in "$contract" "$document" "$example"; do
    [[ -f "$path" ]] || die "missing input: $path"
    tail -c 1 "$path" | cmp -s <(printf '\n') || die "input must end with LF: $path"
    ! grep -nE $'\r|[[:blank:]]$' "$path" >/dev/null || die "CR or trailing whitespace: $path"
done
jq -e -L scripts '
  include "stdlib_regex_progression";
  .format == "tondo-stdlib-owner-contract/1" and .owner == "std.regex"
  and regex_kernel_progression and regex_documentation_metadata
  and .conformance.status == "verified-hosted-vm-adapter-and-native-stdlib-process"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-applicable-pure-core"
  and .implementation.production_vm_registration == "not-claimed"
  and .implementation.native_abi == "not-claimed"
  and .implementation.native_aot_lowering == "not-claimed"
' "$contract" >/dev/null || die "documentation record or promotion boundary differs"
for marker in \
    '## Executable usage guide for `std.regex`' \
    '### Patterns and reuse' \
    '### Unicode and options' \
    '### Captures and UTF-8 spans' \
    '### Lazy iteration and ownership' \
    '### Replacement and errors' \
    '### Limits and costs' \
    '### Executable kernel example' \
    '### Promotion boundary' \
    'RegexLimits::default()' 'RegexSpan::slice' '16.0.0' \
    'Iterator[RegexMatch ! RegexError]' 'does not return the earlier' \
    'system OOM' 'not claimed globally linear' 'stdlib-regex-performance.md' \
    'regex-doc-ok'; do
    grep -Fq "$marker" "$document" || die "document misses: $marker"
done
for function_name in patterns_and_reuse unicode_and_options captures_and_spans \
    lazy_iteration_and_ownership replacement_and_errors limits_and_costs; do
    grep -Fq "fn $function_name()" "$example" || die "example misses: $function_name"
done
output="$(cargo run -q -p tondo-stdlib --example regex_usage --locked)" || die "canonical example failed"
[[ "$output" == "regex-doc-ok" ]] || die "example output differs: $output"
echo "std.regex documentation: OK (canonical Rust example; Unicode, lazy errors, UTF-8 costs and bounded promotion)"
