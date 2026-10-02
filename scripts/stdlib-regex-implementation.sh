#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
if [[ "${TONDO_STDLIB_REGEX_CONTRACT:-$root/testing/stdlib-regex.json}" != "$root/testing/stdlib-regex.json" ]]; then
    echo 'std.regex implementation: evidence must use the canonical owner register' >&2
    exit 1
fi
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" == /* ]] || target_dir="$root/$target_dir"
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-regex-implementation-logs"
mkdir -p "$logs_dir"
scripts/stdlib-regex-implementation-check.sh
if [[ -n "$(git status --porcelain --untracked-files=normal)" && "${TONDO_STDLIB_REGEX_ALLOW_DIRTY:-0}" != 1 ]]; then
    echo 'std.regex implementation: clean tree required; use TONDO_STDLIB_REGEX_ALLOW_DIRTY=1 only for local iteration' >&2
    exit 1
fi
binding() {
    python3 - <<'PY'
import hashlib,json
from pathlib import Path
paths=['Cargo.toml','Cargo.lock','rust-toolchain.toml','crates/tondo-stdlib/Cargo.toml',
       'crates/tondo-stdlib/src/lib.rs','crates/tondo-stdlib/src/regex.rs',
       'crates/tondo-stdlib/src/regex/syntax.rs','crates/tondo-stdlib/src/regex/engine.rs',
       'crates/tondo-stdlib/tests/regex_kernel.rs','testing/stdlib-regex.json',
       'scripts/stdlib_regex_progression.jq','scripts/stdlib-regex-check.sh',
       'scripts/stdlib-regex-implementation-check.sh','scripts/stdlib-regex-implementation.sh']
print(json.dumps([{'path':p,'sha256':'sha256:'+hashlib.sha256(Path(p).read_bytes()).hexdigest()}
                  for p in sorted(paths)],sort_keys=True,separators=(',',':')))
PY
}
before="$(binding)"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib --locked --test regex_kernel > "$logs_dir/tests.log" 2>&1 \
    || { cat "$logs_dir/tests.log" >&2; exit 1; }
grep -Fq '10 passed; 0 failed' "$logs_dir/tests.log" || { echo 'std.regex implementation: incomplete canonical test run' >&2; exit 1; }
CARGO_TARGET_DIR="$target_dir" cargo clippy -q -p tondo-stdlib --locked --all-targets -- -D warnings > "$logs_dir/clippy.log" 2>&1 \
    || { cat "$logs_dir/clippy.log" >&2; exit 1; }
after="$(binding)"
[[ "$before" == "$after" ]] || { echo 'std.regex implementation: inputs changed while checking' >&2; exit 1; }
dirty=false
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || dirty=true
jq -n --arg revision "$(git rev-parse HEAD)" --arg tree "$(git rev-parse HEAD^{tree})" \
    --argjson dirty "$dirty" --argjson sources "$after" \
    --arg rustc "$(rustc --version)" --arg cargo "$(cargo --version)" \
    --arg tests "$(sha256sum "$logs_dir/tests.log" | cut -d' ' -f1)" \
    --arg clippy "$(sha256sum "$logs_dir/clippy.log" | cut -d' ' -f1)" \
    '{format:"tondo-stdlib-regex-implementation-evidence/1",task:"STD-REGEX-IMPL-001",
      status:"kernel-passed",source_revision:$revision,git_tree:$tree,git_dirty:$dirty,
      sources:$sources,toolchain:{rustc:$rustc,cargo:$cargo},unicode:"16.0.0",
      selected_route:"rust-kernel-ordered-thompson-nfa",
      scalar_tests:{package:"tondo-stdlib",test:"regex_kernel",passed:10,failed:0,log_sha256:("sha256:"+$tests)},
      clippy:{package:"tondo-stdlib",all_targets:true,status:"passed",log_sha256:("sha256:"+$clippy)},
      quality_gate:"not-measured-by-kernel-checks",
      public_boundary:{api_promoted:false,production_vm:false,native_abi:false,native_aot:false},
      system_oom_recovery:"not-claimed",timestamps:false,physical_paths:[],addresses:[]}' \
    > "$evidence_dir/stdlib-regex-implementation.json"
echo "std.regex implementation: kernel checks passed; quality separate; report: $evidence_dir/stdlib-regex-implementation.json"
