#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
diff_file="${1:?usage: fast-coverage-check.sh DIFF COVERAGE_JSON [ROOT]}"
coverage_file="${2:?usage: fast-coverage-check.sh DIFF COVERAGE_JSON [ROOT]}"
source_root="${3:-$root}"

[[ -f "$diff_file" ]] || { echo "fast coverage: missing diff: $diff_file" >&2; exit 2; }
[[ -f "$coverage_file" ]] || { echo "fast coverage: missing report: $coverage_file" >&2; exit 2; }

line_records() {
    awk '
        /^\+\+\+ b\// { file=substr($0,7); next }
        /^@@ / {
            h=$0
            sub(/^.*\+/, "", h)
            split(h, parts, " ")
            split(parts[1], span, ",")
            line=span[1]
            remaining=(span[2] == "" ? 1 : span[2])
            next
        }
        /^[+]/ && !/^\+\+\+/ {
            if (remaining > 0) {
                added = substr($0, 2)
                # LLVM can attach zero-count regions to structural-only lines
                # (closing braces and similar formatting). They have no
                # executable statement to cover, so do not turn them into a
                # false changed-line failure.
                if (added !~ /^[[:space:]]*[{}][[:space:]]*$/) {
                    print file "\t" line
                }
                line++
                remaining--
            }
            next
        }
        /^ / { if (remaining > 0) { line++; remaining-- } }
    ' "$diff_file"
}

records="$(mktemp "${TMPDIR:-/tmp}/tondo-fast-coverage.XXXXXX.records")"
coverage_map="$(mktemp "${TMPDIR:-/tmp}/tondo-fast-coverage.XXXXXX.map")"
trap 'rm -f -- "$records" "$coverage_map"' EXIT
line_records > "$records"

# Loading the report once is important: a quality report is large, and one jq
# invocation per changed line turns a small gate into an accidental O(files x
# lines) JSON scan.
jq -r '
    .data[0].files[]
    | .filename as $file
    | .segments[]?
    | select(.[3] == true)
    | [$file, .[0], .[2]]
    | @tsv
' "$coverage_file" > "$coverage_map"

awk -F '\t' -v root="$source_root" '
    NR == FNR {
        key=$1 SUBSEP ($2 + 0)
        count=$3 + 0
        if (!(key in max) || count > max[key]) max[key]=count
        files[$1]=1
        next
    }
    {
        file=$1
        line=$2 + 0
        if (file !~ /\.rs$/) { skipped++; next }
        absolute=root "/" file
        key=absolute SUBSEP line
        if (!(absolute in files) || !(key in max)) { skipped++; next }
        if (max[key] > 0) {
            checked++
        } else {
            printf "fast coverage: uncovered executable line %s:%s\n", file, line > "/dev/stderr"
            failures++
        }
    }
    END {
        printf "fast coverage: checked=%d skipped=%d failures=%d\n", checked, skipped, failures
        exit(failures > 0 ? 1 : 0)
    }
' "$coverage_map" "$records"
