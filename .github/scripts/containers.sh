#!/bin/bash
# Debugging and post-mortems where programs run, as a user's containers and
# CI have them, with Docker:
#
#   containers.sh <tondo> [<out>]
#   containers.sh --across <out> <out of another architecture>
#
# The first runs scenarios 1 to 7 with a static tondo of this machine's
# architecture, and keeps in <out> that tondo and the dump of scenario 1; the
# second runs 8 with two such. Each scenario prints its result; a failure says
# what it saw. Its containers and images are named tondo-scenario-*.
#
#  1. An image from scratch: a panic leaves its dump, says how it leaves the
#     container, `docker cp` takes it out and `tondo dump read` reads it
#     here; with TONDO_DUMP=stderr, from the container's log.
#  2. A read-only file system: said once, and why; the exit is the same; a
#     TONDO_DUMP on a tmpfs takes the dump.
#  3. No tondo inside: the tools reach the program with --via through a
#     tondo copied in; with no TMPDIR it can write, the program says so.
#  4. No ptrace, no capabilities: the same tools work.
#  5. A stripped executable: the stack and the dump keep their lines.
#  6. A container beside another, sharing its processes, as `kubectl debug`
#     makes: the tools find the program through /proc.
#  7. CI: the report a job's summary takes says how to replay a failure.
#  8. Another architecture: a dump of each read by the tondo of the other,
#     the one not of this machine under Docker's emulation.
set -u
failed=0
pass() { echo "ok   $1"; }
fail() { echo "FAIL $1"; shift; printf '     %s\n' "$@" | head -20; failed=1; }
export TONDO_DUMP=off

if [ "$1" = --across ]; then
  here=$(realpath "$2") there=$(realpath "$3")
  platform() { case $(cat "$1/arch") in x86_64) echo linux/amd64 ;; aarch64) echo linux/arm64 ;; esac; }
  read=$("$here/tondo" dump read "$there/crash.dump.json" 2>&1; "$here/tondo" dump read "$there/crash.dump.json" locals 2>&1)
  case $read in
    *P0001*crash.to:6*order*42*) pass "8. a dump of $(cat "$there/arch") reads on $(cat "$here/arch")" ;;
    *) fail "8. a dump of $(cat "$there/arch") read on $(cat "$here/arch") lacks the panic, its line or the bindings" "$read" ;;
  esac
  read=$(docker run --rm --platform "$(platform "$there")" -v "$here:/here:ro" -v "$there:/there:ro" alpine \
    sh -c '/there/tondo dump read /here/crash.dump.json; /there/tondo dump read /here/crash.dump.json locals' 2>&1)
  case $read in
    *P0001*crash.to:6*order*42*) pass "8. a dump of $(cat "$here/arch") reads on $(cat "$there/arch")" ;;
    *) fail "8. a dump of $(cat "$here/arch") read on $(cat "$there/arch") lacks the panic, its line or the bindings" "$read" ;;
  esac
  exit $failed
fi

T=$(realpath "$1")
out=${2:+$(realpath -m "$2")}
W=$(mktemp -d)
cleanup() {
  docker ps -aq --filter name=tondo-scenario- | xargs -r docker rm -f > /dev/null
  docker images -q --filter 'reference=tondo-scenario-*' | sort -u | xargs -r docker rmi -f > /dev/null
  rm -r "$W"
}
trap cleanup EXIT
cd "$W" || exit 1

cat > crash.to <<'EOF'
type Order = { id: Int }

fn main() {
    let order = Order { id: 42 }
    let values = [order.id]
    let _ = values[3]
}
EOF
cat > ticker.to <<'EOF'
import std.time

fn main() {
    var counter = 0
    for {
        counter += 1
        time.sleep(time.milliseconds(20))
    }
}
EOF
# An image from scratch of an executable here: image <name> <file> [env].
image() {
  mkdir -p "img-$1" && cp "$2" "img-$1/program"
  { echo 'FROM scratch'; echo 'COPY program /program'; [ -n "${3:-}" ] && echo "ENV $3"; echo 'ENTRYPOINT ["/program"]'; } > "img-$1/Dockerfile"
  docker build -q ${4:+--platform "$4"} -t "tondo-scenario-$1" "img-$1" > /dev/null
}
# Whether a command succeeds within 20 seconds, as a program that starts.
within() {
  for _ in $(seq 40); do "$@" > /dev/null 2>&1 && return 0; sleep 0.5; done
  return 1
}
# The dump a container's program says it left, taken out: taken <container> <file>.
taken() {
  where=$(docker logs "$1" 2>&1 | sed -n 's/^tondo runtime: a dump of the program is in //p')
  [ -n "$where" ] && docker cp "$1:$where" "$2" 2> /dev/null
}
$T build crash.to --debug --output crash > /dev/null &&
  $T build crash.to --strip --output crash-strip > /dev/null &&
  $T build ticker.to --debug --output ticker > /dev/null || { echo "FAIL the programs do not build"; exit 1; }

# 1
image crash crash
docker run --name tondo-scenario-1 tondo-scenario-crash > /dev/null 2>&1
code=$(docker inspect -f '{{.State.ExitCode}}' tondo-scenario-1)
if taken tondo-scenario-1 s1.dump.json; then
  read=$($T dump read s1.dump.json 2>&1; $T dump read s1.dump.json locals 2>&1)
  [ -n "$out" ] && mkdir -p "$out" && cp "$T" "$out/tondo" && cp s1.dump.json "$out/crash.dump.json" && uname -m > "$out/arch"
  case $read in
    *P0001*crash.to:6*order*42*) pass "1. scratch: the dump leaves with docker cp and reads here" ;;
    *) fail "1. scratch: the dump lacks the panic, its line or the bindings" "$read" ;;
  esac
  if docker logs tondo-scenario-1 2>&1 | grep -q 'it leaves the container with `docker cp'; then
    pass "1. scratch: the program says how its dump leaves the container"
  else
    fail "1. scratch: nothing says how the dump leaves the container" "$(docker logs tondo-scenario-1 2>&1)"
  fi
else
  fail "1. scratch: no dump to take out (exit $code)" "$(docker logs tondo-scenario-1 2>&1)"
fi

docker run --name tondo-scenario-1b -e TONDO_DUMP=stderr tondo-scenario-crash > /dev/null 2>&1
docker logs tondo-scenario-1b > s1b.log 2>&1
read=$($T dump read s1b.log 2>&1; $T dump read s1b.log locals 2>&1)
case $read in
  *P0001*crash.to:6*order*42*) pass "1. TONDO_DUMP=stderr: the dump reads from the container's log" ;;
  *) fail "1. TONDO_DUMP=stderr: no dump read from the log" "$read" ;;
esac

# 2
docker run --name tondo-scenario-2 --read-only tondo-scenario-crash > /dev/null 2>&1
said=$(docker logs tondo-scenario-2 2>&1)
code2=$(docker inspect -f '{{.State.ExitCode}}' tondo-scenario-2)
docker run --name tondo-scenario-2b --read-only --tmpfs /dumps -e TONDO_DUMP=/dumps tondo-scenario-crash > /dev/null 2>&1
if [ "$code2" != "$code" ] || [ "$(echo "$said" | grep -c '^tondo runtime: no dump')" != 1 ] ||
  ! echo "$said" | grep -q 'cannot be made.*give TONDO_DUMP'; then
  fail "2. read-only: not said once and why, or another exit ($code2, not $code)" "$said"
elif ! docker logs tondo-scenario-2b 2>&1 | grep -q 'a dump of the program is in /dumps/'; then
  fail "2. read-only: TONDO_DUMP on a tmpfs takes no dump" "$(docker logs tondo-scenario-2b 2>&1)"
else
  pass "2. read-only: said once, with what to set; the exit is the same"
fi

# 3 and 4: the tools from here, through a tondo copied in, as `docker exec` runs it.
image ticker ticker TONDO_DIAGNOSTICS=1
reach() {
  box=$1
  via="docker exec -i -e PATH=/ $box"
  docker cp "$T" "$box:/tondo" || return 1
  within $T counters program --via "$via" --count 1 || { echo "never reached"; return 1; }
  out=$($T counters program --via "$via" --count 1 --json 2>&1) && echo "$out" | grep -q '"tasks.live"' || { echo "counters: $out"; return 1; }
  out=$($T dump program --via "$via" --output "$box.dump.json" 2>&1) && $T dump read "$box.dump.json" | grep -q ticker.to || { echo "dump: $out"; return 1; }
  out=$($T blocked program --via "$via" 2>&1) || { echo "blocked: $out"; return 1; }
  out=$($T profile program --via "$via" --duration 1 --output "$box.pprof" 2>&1) || { echo "profile: $out"; return 1; }
  out=$(printf 'break ticker.to:6\ncontinue\nprint counter > 0\ndelete 1\n' | $T debug --json --via "$via" program 2>&1)
  echo "$out" | grep -q '"value":"true"' || { echo "debug: $out"; return 1; }
}
docker run -d --name tondo-scenario-3 --tmpfs /tmp tondo-scenario-ticker > /dev/null
if why=$(reach tondo-scenario-3); then
  pass "3. no tondo inside: counters, dump, blocked, profile and debug through --via"
else
  fail "3. no tondo inside: a tool fails through --via" "$why"
fi
docker run -d --name tondo-scenario-3b --read-only tondo-scenario-ticker > /dev/null
said() { docker logs "$1" 2>&1 | grep -q "$2"; }
if within said tondo-scenario-3b 'the tools cannot reach this program:.*TMPDIR'; then
  pass "3. no TMPDIR to write: the program says the tools cannot reach it, and why"
else
  fail "3. no TMPDIR to write: nothing said" "$(docker logs tondo-scenario-3b 2>&1)"
fi
docker run -d --name tondo-scenario-4 --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges tondo-scenario-ticker > /dev/null
if why=$(reach tondo-scenario-4); then
  pass "4. no ptrace, no capabilities: the same tools work"
else
  fail "4. no ptrace: a tool fails" "$why"
fi

# 5
image strip crash-strip
docker run --name tondo-scenario-5 tondo-scenario-strip > /dev/null 2>&1
said=$(docker logs tondo-scenario-5 2>&1)
if echo "$said" | grep -q 'main  crash.to:6' && taken tondo-scenario-5 s5.dump.json &&
  $T dump read s5.dump.json stacks 2>&1 | grep -q 'crash.to:6'; then
  pass "5. stripped: the stack and the dump keep their lines"
else
  fail "5. stripped: lines lost" "$said"
fi

# 6
mkdir -p img-tools && cp "$T" img-tools/tondo && printf 'FROM scratch\nCOPY tondo /tondo\nENTRYPOINT ["/tondo"]\n' > img-tools/Dockerfile
docker build -q -t tondo-scenario-tools img-tools > /dev/null
beside="docker run --rm -i --pid=container:tondo-scenario-3 --tmpfs /tmp tondo-scenario-tools"
listed=$($beside ps 2>&1)
counted=$($beside counters 1 --count 1 --json 2>&1)
debugged=$(printf 'break ticker.to:6\ncontinue\nprint counter > 0\ndelete 1\n' | $beside debug --json 1 2>&1)
if echo "$listed" | grep -q '/program' && echo "$counted" | grep -q '"tasks.live"' &&
  echo "$debugged" | grep -q '"value":"true"'; then
  pass "6. beside the program's container: ps, counters and debug find it"
else
  fail "6. beside the program's container: not found" "$listed" "$counted" "$debugged"
fi

# 7
mkdir -p job/src && printf '[package]\nname = "job"\nversion = "0.1.0"\n' > job/tondo.toml
printf 'pub fn half(n: Int): Int {\n    n / 2\n}\n' > job/src/half.to
printf 'test halves {\n    assert(half(7) == 4)\n}\n' > job/src/half_test.to
(cd job && $T test --record ../recorded --report markdown=../summary.md > /dev/null 2>&1)
if grep -q 'tondo debug --replay ../recorded/job.unit' summary.md 2> /dev/null; then
  pass "7. CI: the summary says how to replay the failure"
else
  fail "7. CI: the summary does not say how to replay" "$(cat summary.md 2>&1)"
fi

exit $failed
