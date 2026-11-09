#!/bin/sh
# Mutation testing of the runtime's core (docs/refactor.md, 0.8): cargo-mutants
# writes the mutants of the scheduler's core, of the locks of `std.sync` and of
# strings and arrays, and each, applied in turn to a copy of the repository,
# must make a test fail: the cases, the runtime's own tests, the orders of
# tests/orders or another test of the toolchain, as those of the tools the
# scheduler serves. One that passes is a missing test, or a change that changes
# nothing, which testing/mutants/explained.txt says why: a line each, the
# mutant as cargo mutants names it without its place, a tab, the line it
# changes as it was, a tab, and why. The first argument is the slice of them to
# try, as `3/30`, so that each night tries another within its time; the second,
# the seconds it may take. It prints each that survived, with its diff, and
# ends with a summary; it fails only when the unmutated tests fail.
set -eu
slice=${1:-1/1}
budget=${2:-3000}
root=$(git rev-parse --show-toplevel)
work=$(mktemp -d)
git -C "$root" archive HEAD | tar -x -C "$work"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target/mutants}
export TONDO_RUNTIME_FAST=1
tasks='in (work_until|steal|enqueue|wake_idle|new_task|first_priority|recycle|Wait::parked|Wait::block|notify_with|schedule|ending|timers_ended|Few<T>::partition_dedup_len)$'
locks='in (try_take|grant|ungrant)$'
strings='in (Builder::(extend|grow|finish)|append|tondo_rt_str_[a-z_]+|tondo_rt_array_[a-z_]+|tondo_rt_sb_[a-z_]+)$'
cd "$root"
cargo mutants --list --json --diff -p tondo-runtime \
  --file 'crates/tondo-runtime/src/tasks/*.rs' --file crates/tondo-runtime/src/strings.rs \
  --file crates/tondo-runtime/src/arrays.rs --file crates/tondo-runtime/src/builder.rs \
  --file crates/tondo-runtime/src/sync.rs \
  --re "$tasks" --re "$locks" --re "$strings" > "$work/mutants.json"
cd "$work"
python3 - "$slice" "$budget" "$work/mutants.json" "$root/testing/mutants/explained.txt" <<'EOF'
import json, os, subprocess, sys, time

slice, budget, listed, explained = sys.argv[1], float(sys.argv[2]), sys.argv[3], sys.argv[4]
part, parts = (int(n) for n in slice.split("/"))
started = time.monotonic()
known = set()
if os.path.exists(explained):
    for line in open(explained):
        if line.strip() and not line.startswith("#"):
            key, changed, _ = line.rstrip("\n").split("\t", 2)
            known.add((key, changed))
mutants = [m for i, m in enumerate(json.load(open(listed))) if i % parts == part - 1]

tondo = os.path.join(os.environ["CARGO_TARGET_DIR"], "debug", "tondo")

def tests(timeout):
    commands = [
        ["cargo", "test", "-q", "-p", "tondo", "--test", "cases", "cases"],
        ["cargo", "test", "-q", "-p", "tondo-runtime", "--lib"],
        [tondo, "test", "tests/orders", "--schedules", "all"],
        ["cargo", "test", "-q", "-p", "tondo", "--", "--exact", "--skip", "cases", "--skip", "projects"],
    ]
    for command in commands:
        try:
            if subprocess.run(command, capture_output=True, timeout=timeout).returncode != 0:
                return "failed"
        except subprocess.TimeoutExpired:
            return "timeout"
    return 0

if tests(3600) != 0:
    print("the tests fail with no mutant: nothing to measure")
    sys.exit(1)
outcomes = {"caught": 0, "missed": 0, "unviable": 0, "explained": 0}
for mutant in mutants:
    if time.monotonic() - started > budget:
        break
    # Without its place, which moves with every edit above it, but with the
    # line it changes, as one function may have the same change in two.
    name, key = mutant["name"], mutant["name"].split(": ", 1)[1]
    changed = next(line[1:].strip() for line in mutant["diff"].split("\n") if line.startswith("-") and not line.startswith("---"))
    path = mutant["file"]
    original = open(path).read()
    diff = mutant["diff"].split("\n")
    diff[1] = "+++ " + path
    applied = subprocess.run(["patch", "-s", "-p0", path], input="\n".join(diff), text=True, capture_output=True)
    if applied.returncode != 0:
        outcomes["unviable"] += 1
        continue
    built = subprocess.run(["cargo", "build", "-q", "-p", "tondo", "--tests"], capture_output=True)
    if built.returncode != 0:
        outcomes["unviable"] += 1
    else:
        result = tests(900)
        if result != 0:
            outcomes["caught"] += 1
        elif (key, changed) in known:
            outcomes["explained"] += 1
        else:
            outcomes["missed"] += 1
            print(f"survived: {name}\n{mutant['diff']}\n", flush=True)
    open(path, "w").write(original)
tried = sum(outcomes.values())
print(f"slice {slice}: {tried} of {len(mutants)} mutants tried, " + ", ".join(f"{v} {k}" for k, v in outcomes.items()))
EOF
