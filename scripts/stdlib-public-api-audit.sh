#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

config="${TONDO_PUBLIC_API_CONFIG:-testing/stdlib-public-api-config.json}"
matrix="${TONDO_PUBLIC_API_MATRIX:-testing/stdlib-public-api.json}"

die() {
    echo "stdlib public API audit: $*" >&2
    exit 1
}

[[ -f "$config" ]] || die "missing audit configuration: $config"
jq -e '
    .format == "tondo-stdlib-public-api-audit-config/1"
    and .edition == "0.1"
    and .phase == "STD-0.1A"
    and (.owners | length) == 20
    and ([.owners[].id] | unique | length) == 20
    and all(.owners[];
        (.id | test("^std\\.[a-z]+$"))
        and (.contract | endswith(".md"))
        and (.hir | length > 0)
        and (.lowering | length > 0)
        and (.case.path | length > 0)
        and (.case.kind | ["runtime", "compile", "runner-source", "build-only"] | index(.) != null)
        and (.runtime.kind | ["host", "vm", "vm-inline", "not-applicable"] | index(.) != null)
        and ((.runtime.symbols // {}) | type) == "object"
        and ((.hir_symbols // {}) | type) == "object"
        and ((.lowering_symbols // {}) | type) == "object"
        and ((if has("exact_surface_complete") then .exact_surface_complete else true end) | type) == "boolean"
        and ((.exact_signatures // []) | type) == "array"
        and all((.exact_signatures // [])[]; type == "string" and length > 0)
        and (if (has("exact_surface_complete") and (.exact_surface_complete == false)) then has("exact_signatures") else true end)
        and (if .runtime.kind == "not-applicable" then (.runtime.paths | length) == 0 and (.runtime.reason | type) == "string" and (.runtime.reason | length > 0) else (.runtime.paths | length > 0) end)
    )
' "$config" >/dev/null || die "invalid audit configuration"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-public-api.XXXXXX")"
# The host runner owns temporary-directory cleanup; avoid recursive deletion
# from this audit so it remains safe to invoke inside a shared test process.
rows_ndjson="$tmp_dir/rows.ndjson"
: > "$rows_ndjson"

path_exists() {
    [[ -e "$root/$1" ]]
}

all_paths_exist() {
    local paths_json="$1"
    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        path_exists "$path" || return 1
    done < <(jq -r '.[]' <<< "$paths_json")
}

all_paths_contain() {
    local paths_json="$1"
    local needle="$2"
    local path
    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        grep -Fq -- "$needle" "$root/$path" && return 0
    done < <(jq -r '.[]' <<< "$paths_json")
    return 1
}

all_paths_contain_any() {
    local paths_json="$1"
    local needles_json="$2"
    local path needle
    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        while IFS= read -r needle; do
            [[ -n "$needle" ]] || continue
            grep -Fq -- "$needle" "$root/$path" && return 0
        done < <(jq -r '.[]' <<< "$needles_json")
    done < <(jq -r '.[]' <<< "$paths_json")
    return 1
}

stage_symbols_for() {
    local owner_json="$1"
    local stage="$2"
    local name="$3"
    local operation="$4"
    jq -c --arg stage "$stage" --arg name "$name" --arg operation "$operation" \
        '((.[$stage] // {})[$name] // [$operation])' <<< "$owner_json"
}

has_prefix() {
    local value="$1"
    local prefixes_json="$2"
    local prefix
    while IFS= read -r prefix; do
        [[ -n "$prefix" && "$value" == "$prefix"* ]] && return 0
    done < <(jq -r '.[]' <<< "$prefixes_json")
    return 1
}

has_forbidden_prefix() {
    local value="$1"
    local prefixes_json="$2"
    local prefix
    while IFS= read -r prefix; do
        [[ -n "$prefix" && "$value" == "$prefix"* ]] && return 0
    done < <(jq -r '.[]' <<< "$prefixes_json")
    return 1
}

canonical_call_for() {
    local owner_json="$1"
    local name="$2"
    jq -r --arg name "$name" '
        (.case.canonical_calls // [])
        | map(select(.from == $name) | .to)
        | first // empty
    ' <<< "$owner_json"
}

runtime_symbols_for() {
    local symbols_json="$1"
    local name="$2"
    jq -c --arg name "$name" '.[$name] // []' <<< "$symbols_json"
}

operation_name() {
    local name="$1"
    name="${name##*.}"
    name="${name%%[*}"
    printf '%s' "$name"
}

normalized_symbol() {
    sed -E 's/\[[^]]*\]//g' <<< "$1"
}

extract_signatures() {
    local owner_json="$1"
    local contract section
    contract="$(jq -r '.contract' <<< "$owner_json")"
    section="$(jq -r '.section' <<< "$owner_json")"
    if [[ "$section" == "*" ]]; then
        awk '
            /^pub (unsafe )?fn / {
                line=$0
                sub(/[[:space:]]*\/\/.*$/, "", line)
                sub(/[[:space:]]+$/, "", line)
                print NR "\t" line
            }
        ' "$root/$contract"
    else
        awk -v wanted="$section" '
            BEGIN { in_section=0 }
            /^## / { in_section=(index($0, wanted) > 0) }
            in_section && /^pub (unsafe )?fn / {
                line=$0
                sub(/[[:space:]]*\/\/.*$/, "", line)
                sub(/[[:space:]]+$/, "", line)
                print NR "\t" line
            }
        ' "$root/$contract"
    fi
}

emit_owner_rows() {
    local owner_json="$1"
    local owner contract section include exclude case_path case_kind exact_surface_complete exact_signatures
    owner="$(jq -r '.id' <<< "$owner_json")"
    contract="$(jq -r '.contract' <<< "$owner_json")"
    section="$(jq -r '.section' <<< "$owner_json")"
    include="$(jq -c '.include' <<< "$owner_json")"
    exclude="$(jq -c '.exclude' <<< "$owner_json")"
    case_path="$(jq -r '.case.path' <<< "$owner_json")"
    case_kind="$(jq -r '.case.kind' <<< "$owner_json")"
    exact_surface_complete="$(jq -r 'if has("exact_surface_complete") then .exact_surface_complete else true end' <<< "$owner_json")"
    exact_signatures="$(jq -c '.exact_signatures // []' <<< "$owner_json")"
    local hir lowering runtime runtime_kind runtime_reason runtime_symbols
    hir="$(jq -c '.hir' <<< "$owner_json")"
    lowering="$(jq -c '.lowering' <<< "$owner_json")"
    runtime="$(jq -c '.runtime.paths' <<< "$owner_json")"
    runtime_kind="$(jq -r '.runtime.kind' <<< "$owner_json")"
    runtime_reason="$(jq -r '.runtime.reason // empty' <<< "$owner_json")"
    runtime_symbols="$(jq -c '.runtime.symbols // {}' <<< "$owner_json")"

    if ! path_exists "$contract"; then
        die "$owner contract path does not exist: $contract"
    fi
    all_paths_exist "$hir" || die "$owner HIR evidence path is missing"
    all_paths_exist "$lowering" || die "$owner lowering evidence path is missing"
    if [[ "$runtime_kind" != "not-applicable" ]]; then
        all_paths_exist "$runtime" || die "$owner runtime evidence path is missing"
    fi
    path_exists "$case_path" || die "$owner public-case path is missing: $case_path"

    local line signature name operation symbol canonical_call hir_needles lowering_needles runtime_needles
    local -a missing
    while IFS=$'\t' read -r line signature; do
        [[ -n "$signature" ]] || continue
        name="${signature#pub }"
        name="${name#unsafe }"
        name="${name#fn }"
        name="${name%%(*}"
        if [[ "$name" == *' '* ]]; then
            name="${name%% *}"
        fi
        name="${name%%[*}"
        if [[ "$(jq 'length' <<< "$include")" -gt 0 ]] && ! has_prefix "$name" "$include"; then
            continue
        fi
        if [[ "$(jq 'length' <<< "$exclude")" -gt 0 ]] && has_forbidden_prefix "$name" "$exclude"; then
            continue
        fi

        operation="$(operation_name "$name")"
        symbol="$(normalized_symbol "$owner.$name")"
        canonical_call="$(canonical_call_for "$owner_json" "$name")"
        [[ -n "$canonical_call" ]] || canonical_call="$operation"
        missing=()

        hir_needles="$(stage_symbols_for "$owner_json" hir_symbols "$owner.$name" "$operation")"
        lowering_needles="$(stage_symbols_for "$owner_json" lowering_symbols "$owner.$name" "$operation")"

        if ! grep -Fq -- "$signature" "$root/$contract"; then
            missing+=("contract-signature-drift")
        fi
        if [[ "$exact_surface_complete" != true ]] &&
            ! jq -e --arg name "$name" 'index($name) != null' <<< "$exact_signatures" >/dev/null; then
            missing+=("exact-signature-shape")
        fi
        if ! all_paths_contain_any "$hir" "$hir_needles"; then
            missing+=("hir-symbol")
        fi
        if ! all_paths_contain_any "$lowering" "$lowering_needles"; then
            missing+=("lowering-symbol")
        fi
        runtime_needles="$(runtime_symbols_for "$runtime_symbols" "$owner.$name")"
        if [[ "$(jq 'length' <<< "$runtime_needles")" -eq 0 ]]; then
            runtime_needles="$(jq -cn --arg operation "$operation" '[ $operation ]')"
        fi

        if [[ "$runtime_kind" == "not-applicable" ]]; then
            [[ -n "$runtime_reason" ]] || missing+=("runtime-not-applicable-reason")
        elif ! all_paths_contain_any "$runtime" "$runtime_needles"; then
            if [[ "$runtime_kind" == "host" ]]; then
                missing+=("host-symbol")
            else
                missing+=("vm-symbol")
            fi
        fi

        case "$case_kind" in
            runtime)
                [[ "$case_path" == tests/runtime/*.to ]] || missing+=("invalid-runtime-case")
                ;;
            compile)
                [[ "$case_path" != docs/* && "$case_path" == crates/*/tests/* ]] || missing+=("invalid-compile-case")
                ;;
            runner-source)
                [[ "$case_path" == crates/tondo-compiler/src/driver.rs ]] || missing+=("invalid-runner-case")
                ;;
            build-only)
                [[ "$runtime_kind" == "not-applicable" && "$case_path" == crates/* && "$case_path" != docs/* ]] || missing+=("invalid-build-only-case")
                ;;
            *)
                missing+=("unknown-case-kind")
                ;;
        esac
        # Avoid a grep -v | grep -q pipeline here: with pipefail enabled the
        # producer can lose a SIGPIPE after the consumer finds an early match,
        # making an otherwise stable audit nondeterministically report a gap.
        if ! awk -v needle="$canonical_call" \
            '!/^[[:space:]]*\/\// && index($0, needle) { found=1 } END { exit found ? 0 : 1 }' \
            "$root/$case_path"; then
            missing+=("public-case-call")
        fi
        if [[ "$case_path" == docs/* || "$case_kind" == "documentation" ]]; then
            missing+=("documentation-is-not-public-case")
        fi

        local missing_json status
        missing_json="$(printf '%s\n' "${missing[@]}" | jq -R -s 'split("\n") | map(select(length > 0)) | unique')"
        if [[ "${#missing[@]}" -eq 0 ]]; then status="verified"; else status="gap"; fi
        jq -n \
            --arg owner "$owner" \
            --arg contract "$contract" \
            --argjson line "$line" \
            --arg signature "$signature" \
            --arg symbol "$symbol" \
            --arg operation "$operation" \
            --arg case_path "$case_path" \
            --arg case_kind "$case_kind" \
            --arg call "$canonical_call" \
            --arg runtime_kind "$runtime_kind" \
            --arg runtime_reason "$runtime_reason" \
            --argjson hir "$hir" \
            --argjson lowering "$lowering" \
            --argjson runtime "$runtime" \
            --argjson hir_symbols "$hir_needles" \
            --argjson lowering_symbols "$lowering_needles" \
            --argjson runtime_symbols "$runtime_needles" \
            --argjson missing "$missing_json" \
            --arg status "$status" \
            '{id:($owner+":"+($line|tostring)), owner:$owner, contract:$contract, line:$line, signature:$signature, symbol:$symbol, operation:$operation, evidence:{hir:{paths:$hir,symbol:$operation,symbols:$hir_symbols},lowering:{paths:$lowering,symbol:$operation,symbols:$lowering_symbols},host_vm:{kind:$runtime_kind,paths:$runtime,reason:(if $runtime_reason == "" then null else $runtime_reason end),symbol:$symbol,symbols:$runtime_symbols},public_case:{path:$case_path,kind:$case_kind,call:$call,bootstrap_alias:false}},missing:$missing,status:$status}' \
            >> "$rows_ndjson"
    done < <(extract_signatures "$owner_json")
}

while IFS= read -r owner_json; do
    emit_owner_rows "$owner_json"
done < <(jq -c '.owners[]' "$config")

generate_matrix() {
    local output="$1"
    jq -n --slurpfile rows "$rows_ndjson" --slurpfile config_json "$config" '
        ($rows) as $all_rows
        | ($config_json[0]) as $config
        | ($config.owners | map({id,contract,section,exact_surface_complete:(if has("exact_surface_complete") then .exact_surface_complete else true end),exact_signatures:(.exact_signatures // []),case:.case,hir,lowering,runtime})) as $owner_config
        | ($owner_config | map(. as $owner |
            ($all_rows | map(select(.owner == $owner.id))) as $owner_rows
            | [
                (if ($owner_rows | length) == 0 and $owner.runtime.kind != "not-applicable" then "no-callable-signatures-indexed" else empty end),
                (if ($owner.case.kind == "runtime" and (($owner.case.path | startswith("tests/runtime/")) | not)) then "invalid-runtime-case" else empty end),
                (if ($owner.case.kind == "compile" and ((($owner.case.path | startswith("crates/")) and ($owner.case.path | contains("/tests/"))) | not)) then "invalid-compile-case" else empty end),
                (if ($owner.case.kind == "runner-source" and $owner.case.path != "crates/tondo-compiler/src/driver.rs") then "invalid-runner-case" else empty end),
                (if ($owner.case.kind == "build-only" and (($owner.runtime.kind != "not-applicable") or (($owner.case.path | startswith("crates/")) | not) or ($owner.case.path | startswith("docs/")))) then "invalid-build-only-case" else empty end),
                (if ($owner.case.path | startswith("docs/")) then "documentation-is-not-public-case" else empty end)
            ] as $owner_missing
            | . + {
                signature_count: ($owner_rows | length),
                verified_count: ($owner_rows | map(select(.status == "verified")) | length),
                gap_count: ($owner_rows | map(select(.status == "gap")) | length),
                owner_missing: $owner_missing,
                status: (if (($owner_rows | any(.status == "gap")) or (($owner_missing | length) > 0)) then "open-gaps" else "verified" end)
            })) as $owners
        | {
            format:"tondo-stdlib-public-api-audit/1",
            edition:$config.edition,
            phase:$config.phase,
            config:"testing/stdlib-public-api-config.json",
            rules:{
                one_owner_per_signature:true,
                required_stages:["contract","hir","lowering","host_vm","public_case"],
                public_case_kinds:["runtime","compile","runner-source","build-only"],
                documentation_is_not_public_case:true,
                bootstrap_aliases:false,
                exact_signature_shape_required:true,
                strict_mode:"fails-on-any-gap"
            },
            status:(if (($all_rows | any(.status == "gap")) or ($owners | any(.owner_missing | length > 0))) then "open-gaps" else "verified" end),
            owners:$owners,
            rows:($all_rows | sort_by([.owner,.line])),
            summary:{owners:($owners|length), signatures:($all_rows|length), verified:($all_rows|map(select(.status=="verified"))|length), signature_gaps:($all_rows|map(select(.status=="gap"))|length), owner_gaps:($owners|map(select(.owner_missing|length>0))|length), gaps:(($all_rows|map(select(.status=="gap"))|length) + ($owners|map(select(.owner_missing|length>0))|length))}
        }
    ' > "$output"
}

validate_matrix() {
    local input="$1"
    local strict="$2"
    [[ -f "$input" ]] || die "missing generated matrix: $input"
    tail -c 1 "$input" | cmp -s <(printf '\n') || die "matrix must end with LF"
    ! grep -nE $'\r|[[:blank:]]$' "$input" >/dev/null || die "matrix contains CR or trailing whitespace"
    jq -e '
        .format == "tondo-stdlib-public-api-audit/1"
        and .edition == "0.1"
        and .phase == "STD-0.1A"
        and .config == "testing/stdlib-public-api-config.json"
        and .rules.one_owner_per_signature == true
        and .rules.documentation_is_not_public_case == true
        and .rules.bootstrap_aliases == false
        and .rules.exact_signature_shape_required == true
        and .rules.strict_mode == "fails-on-any-gap"
        and (.owners | length) == 20
        and ([.owners[].id] | unique | length) == 20
        and ([.rows[].id] | unique | length) == (.rows | length)
        and (.summary.signatures == (.rows | length))
        and (.summary.verified + .summary.signature_gaps == .summary.signatures)
        and (.status == (if .summary.gaps > 0 then "open-gaps" else "verified" end))
        and all(.rows[];
            (.owner | startswith("std."))
            and (.signature | startswith("pub "))
            and (.symbol | startswith("std."))
            and (.evidence.hir.paths | length > 0)
            and (.evidence.lowering.paths | length > 0)
            and (.evidence.hir.symbols | type) == "array"
            and (.evidence.lowering.symbols | type) == "array"
            and (.evidence.host_vm.symbols | type) == "array"
            and (.evidence.public_case.bootstrap_alias == false)
            and (.status == (if (.missing | length) == 0 then "verified" else "gap" end))
        )
        and all(.owners[];
            (.owner_missing | type) == "array"
            and (.exact_surface_complete | type) == "boolean"
            and (.exact_signatures | type) == "array"
            and (.status == (if ((.owner_missing | length) > 0 or .gap_count > 0) then "open-gaps" else "verified" end))
            and (if .case.kind == "runtime" then ((.case.path | startswith("tests/runtime/")) or any(.owner_missing[]; . == "invalid-runtime-case"))
                 elif .case.kind == "compile" then (((.case.path | startswith("crates/")) and (.case.path | contains("/tests/"))) or any(.owner_missing[]; . == "invalid-compile-case"))
                 elif .case.kind == "runner-source" then (.case.path == "crates/tondo-compiler/src/driver.rs" or any(.owner_missing[]; . == "invalid-runner-case"))
                 elif .case.kind == "build-only" then (((.case.path | startswith("crates/")) and (.runtime.kind == "not-applicable")) or any(.owner_missing[]; . == "invalid-build-only-case"))
                 else false end)
            and ((.case.path | startswith("docs/") | not) or any(.owner_missing[]; . == "documentation-is-not-public-case"))
        )
    ' "$input" >/dev/null || die "matrix schema or status is invalid"

    if [[ "$strict" == true ]]; then
        local gaps
        gaps="$(jq -r '.summary.gaps' "$input")"
        [[ "$gaps" == 0 ]] || die "strict mode found $gaps public API gaps"
    fi
}

generated="$tmp_dir/generated.json"
generate_matrix "$generated"

case "${1:---check}" in
    --write)
        cp "$generated" "$matrix"
        validate_matrix "$matrix" false
        ;;
    --check)
        [[ -f "$matrix" ]] || die "missing checked-in matrix: $matrix (run --write)"
        cmp -s "$generated" "$matrix" || die "checked-in matrix is stale (run --write)"
        validate_matrix "$matrix" false
        ;;
    --strict)
        validate_matrix "$generated" true
        ;;
    *)
        die "usage: $0 [--write|--check|--strict]"
        ;;
esac

jq -r '"stdlib public API audit: " + .status + " (" + (.summary.verified|tostring) + "/" + (.summary.signatures|tostring) + " signatures verified; " + (.summary.gaps|tostring) + " gaps)"' "$matrix" 2>/dev/null || \
    jq -r '"stdlib public API audit: " + .status + " (" + (.summary.verified|tostring) + "/" + (.summary.signatures|tostring) + " signatures verified; " + (.summary.gaps|tostring) + " gaps)"' "$generated"
