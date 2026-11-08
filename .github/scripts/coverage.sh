#!/bin/sh
# What the toolchain's tests run of the runtime and of the standard
# library's Tondo code (docs/refactor.md, 0.1), on Linux: every program the
# tests link carries a runtime built with `-C instrument-coverage`, which
# writes its counts as it ends, and the cases count the lines of the
# standard library they run. The first argument is the directory for the
# results: runtime.txt, the runtime's lines, functions and branches by file,
# and std.txt, the standard library's lines and functions by file, then
# those no case ran. Needs llvm-profdata and llvm-cov of the LLVM rustc
# uses, as `rustup component add llvm-tools` gives, and a C compiler.
set -u
out=$(realpath -m "$1")
root=$(git rev-parse --show-toplevel)
cd "$root"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target/coverage}
export TONDO_RUNTIME_COVERAGE=1 TONDO_RUNTIME_FAST=1
rm -rf "$out"
mkdir -p "$out/profiles" "$out/std"
export LLVM_PROFILE_FILE="$out/profiles/%p%c.profraw"
step() {
  name=$1
  shift
  "$@" > "$out/$name.log" 2>&1
  echo "$name $?" >> "$out/steps.txt"
}
# Some tests fail with a runtime that counts, as those that compare a
# program's size or its symbols: what they run is counted all the same.
step workspace cargo test --workspace --no-fail-fast
step cases env TONDO_CASES_COVERAGE="$out/std" cargo test -p tondo --test cases
step debug env TONDO_CASES_DEBUG=1 cargo test -p tondo --test cases cases
step replay env TONDO_CASES_REPLAY=1 cargo test -p tondo --test cases cases
step live env TONDO_CASES_LIVE=1 cargo test -p tondo --test cases cases
step orders "$CARGO_TARGET_DIR/debug/tondo" test tests/orders --schedules all
# The whole runtime in one executable, which the counts of every program map
# onto, linked by the C compiler, which keeps the map of what each counter
# counts that a program leaves out.
archive=$(find "$CARGO_TARGET_DIR" -name libtondo_runtime.a -path "*runtime-coverage*" | head -n 1)
arguments=$(cat "$(dirname "$(dirname "$(dirname "$(dirname "$archive")")")")/runtime-link-args.txt")
printf 'int main(void) { return 0; }\n' > "$out/whole.c"
# shellcheck disable=SC2086
step link cc -o "$out/whole" "$out/whole.c" -Wl,--whole-archive "$archive" -Wl,--no-whole-archive $arguments \
  -Wl,--unresolved-symbols=ignore-all
find "$out/profiles" -name '*.profraw' > "$out/profiles.txt"
step merge llvm-profdata merge -sparse -f "$out/profiles.txt" -o "$out/runtime.profdata"
llvm-cov report "$out/whole" -instr-profile="$out/runtime.profdata" \
  -ignore-filename-regex='(\.cargo|rustc|vendor)/' > "$out/runtime.txt" 2>&1
python3 - "$out/std" > "$out/std.txt" <<'EOF'
import json, os, struct, sys
from collections import defaultdict

# Each case left `counted`, what each counter of its build counts, and in
# `counts/` what its runs wrote: the standard library's lines and functions
# that ran, of those the builds had.
root = sys.argv[1]
lines, functions = defaultdict(dict), defaultdict(dict)
for suite in sorted(os.listdir(root)):
    for case in sorted(os.listdir(os.path.join(root, suite))):
        here = os.path.join(root, suite, case)
        if not os.path.exists(os.path.join(here, "counted")):
            continue
        counted = json.load(open(os.path.join(here, "counted")))
        totals = [0] * len(counted)
        for name in os.listdir(os.path.join(here, "counts")):
            data = open(os.path.join(here, "counts", name), "rb").read()
            for i in range(min(len(counted), len(data) // 8)):
                totals[i] += struct.unpack_from("<Q", data, 8 * i)[0]
        for entry, total in zip(counted, totals):
            if not entry[1].startswith("<std>/"):
                continue
            if entry[0] == "line":
                lines[entry[1]][entry[2]] = lines[entry[1]].get(entry[2], 0) + total
            elif entry[0] == "function":
                key = (entry[2], entry[3])
                functions[entry[1]][key] = functions[entry[1]].get(key, 0) + total


def runs(numbers):
    out, start, last = [], None, None
    for n in sorted(numbers):
        if start is not None and n == last + 1:
            last = n
            continue
        if start is not None:
            out.append(f"{start}" if start == last else f"{start}-{last}")
        start = last = n
    if start is not None:
        out.append(f"{start}" if start == last else f"{start}-{last}")
    return out


found = hit = 0
print(f"{'file':44} {'lines':>18} {'functions':>12}")
for file in sorted(set(lines) | set(functions)):
    ran = sum(1 for v in lines[file].values() if v)
    called = sum(1 for v in functions[file].values() if v)
    found, hit = found + len(lines[file]), hit + ran
    share = f"{100 * ran / len(lines[file]):.1f}% {ran}/{len(lines[file])}" if lines[file] else "-"
    print(f"{file:44} {share:>18} {called:>5}/{len(functions[file])}")
print(f"{'total':44} {100 * hit / max(found, 1):.1f}% {hit}/{found}")
for file in sorted(functions):
    never = sorted(key for key, calls in functions[file].items() if calls == 0)
    if never:
        print(f"\n{file}, functions no case called:")
        for line, name in never:
            print(f"  {line} {name}")
for file in sorted(lines):
    never = runs(line for line, count in lines[file].items() if count == 0)
    if never:
        print(f"\n{file}, lines no case ran: {', '.join(never)}")
EOF
echo finished >> "$out/steps.txt"
