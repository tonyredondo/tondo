#!/bin/bash
# What CI and the Nightly run, on this machine, in containers, on the last
# commit of this repository, so that a failure shows before a push:
#
#   .github/scripts/local.sh <out> [services] [nocc] [musl] [emulated] [emuloop] [stress] [sweep] [editor]
#
# Each stage prints its status and leaves its output in <out>/<stage>.txt.
# `musl`, `emulated` and `emuloop` have targets of their own, so they can run
# beside the others in another call. It works in a clone in
# ${TONDO_CI_DIR:-~/.cache/tondo-ci}, with containers and ports of its own.
#
# `stress` runs the tests that race on what a runner has (STRESS_ROUNDS), in a
# container with no init, which collects no orphans, on four cores, on one and on
# a quarter of one, and then the cases with their tasks in the order of a seed
# for each round: what a runner rarely shows comes out in minutes. `emuloop` runs them under
# emulation (EMU_ROUNDS), on EMU_CPUS cores: all of them, and EMU_INIT= with no
# init, is what brings races out. `sweep` runs the tools over every case, as the
# Nightly does, and `editor` the extension in a real VS Code, in a container, so
# that it touches nothing of this machine's. macOS is not here.
R=$(git rev-parse --show-toplevel) || exit 1
S=${TONDO_CI_DIR:-$HOME/.cache/tondo-ci}; C=$S/clone
out=$(realpath -m "$1"); shift; mkdir -p "$out" "$S/tmp" "$S/emu-target" "$S/alpine-target"
[ -d "$C/.git" ] || git clone -q "$R" "$C"
cd "$C" || exit 1
git fetch -q "$R" HEAD && git checkout -q --detach FETCH_HEAD || exit 1
git log --oneline -1
export TMPDIR=$S/tmp RUNNER_TEMP=$S/tmp
me="$(id -u):$(id -g)"

# The images the stages run in, with what the tests use.
image() {
  docker image inspect "$1" > /dev/null 2>&1 && return
  printf 'FROM debian:stable-slim\nRUN apt-get update -qq && apt-get install -y -qq gcc git curl procps gdb python3 > /dev/null\n' |
    docker build -q --platform "$2" -t "$1" - > /dev/null
}

services() {
  docker rm -f tondo-ci-pg tondo-ci-mysql tondo-ci-pebble > /dev/null 2>&1
  docker run -d --name tondo-ci-pg -p 127.0.0.1:15432:5432 -e POSTGRES_PASSWORD=secret -e POSTGRES_DB=tondo postgres:18 > /dev/null
  docker run -d --name tondo-ci-mysql -p 127.0.0.1:13306:3306 -e MYSQL_ROOT_PASSWORD=secret -e MYSQL_DATABASE=tondo mysql:8.4 > /dev/null
  docker run -d --name tondo-ci-pebble --network host -e PEBBLE_VA_NOSLEEP=1 ghcr.io/letsencrypt/pebble > /dev/null
  sleep 25
  docker cp tondo-ci-pebble:/test/certs/pebble.minica.pem "$S/tmp/pebble.pem"
  TONDO_TEST_POSTGRES=postgres://postgres:secret@localhost:15432/tondo \
  TONDO_TEST_MYSQL=mysql://root:secret@localhost:13306/tondo \
  TONDO_TEST_ACME=https://localhost:14000/dir TONDO_TEST_ACME_TRUST=$S/tmp/pebble.pem \
    cargo test --workspace --test sql --test acme
  local status=$?
  docker rm -f tondo-ci-pg tondo-ci-mysql tondo-ci-pebble > /dev/null 2>&1
  return $status
}

nocc() {
  TONDO_CC=cc cargo test --workspace --test cases cases || return 1
  TONDO_CASES_STATIC=1 cargo test --workspace --test cases cases || return 1
  cases=$(cargo test --workspace --test cases --no-run 2>&1 | grep -o 'target/debug/deps/cases-[0-9a-f]*')
  run="docker run --rm -u $me -e HOME=/tmp -v $PWD:$PWD -w $PWD"
  $run debian:stable-slim sh -c "! command -v cc && $cases cases projects" || return 1
  $run -e TONDO_CASES_STATIC=1 debian:stable-slim sh -c "! command -v cc && $cases cases"
}

musl() {
  mkdir -p ~/.cargo/registry ~/.cargo/git
  docker run --rm -u 0 -v "$PWD:/work" -w /work -e CARGO_INCREMENTAL=0 -e CARGO_TARGET_DIR=/alpine-target \
    -v "$S/alpine-target:/alpine-target" \
    -v "$HOME/.cargo/registry:/usr/local/cargo/registry" -v "$HOME/.cargo/git:/usr/local/cargo/git" rust:alpine sh -c \
    "apk add --no-cache musl-dev zlib-dev zlib-static git curl tar &&
     (rustup show active-toolchain || rustup toolchain install) &&
     # What the tests left, which the last run gave this machine's user: as
     # root's again, a directory of dumps is private to who runs them.
     (chown -R 0:0 /alpine-target/tmp 2>/dev/null; true) &&
     cargo test --workspace && TONDO_CASES_STATIC=1 cargo test --workspace --test cases cases;
     status=\$?; chown -R $me /alpine-target /usr/local/cargo/registry /usr/local/cargo/git; exit \$status"
}

emulated() {
  set -o pipefail
  . .github/scripts/across.sh aarch64
  mkdir -p target
  CARGO_TARGET_DIR=$S/emu-target cargo test --workspace --target aarch64-unknown-linux-gnu --no-run --message-format=json |
    jq -r 'select(.profile.test == true and .executable != null)
      | (.manifest_path | rtrimstr("Cargo.toml")) + " " + .executable' > target/emulated || return 1
  image tondo-emu linux/arm64
  docker run --rm --platform linux/arm64 -v "$PWD:$PWD" -v "$S/emu-target:$S/emu-target" -v "$S/tmp:$S/tmp" -w "$PWD" \
    -e OWNER="$me" -e TMPDIR="$S/tmp" tondo-emu .github/scripts/emulated.sh
}

editor() {
  cargo build -p tondo || return 1
  (cd editors/vscode && npm ci && npm run compile) || return 1
  printf 'FROM node:22-trixie-slim\nRUN apt-get update -qq && apt-get install -y -qq xvfb xauth libnss3 libatk1.0-0 libatk-bridge2.0-0 libcups2 libdrm2 libgtk-3-0 libgbm1 libasound2 libxss1 libxkbfile1 libsecret-1-0 ca-certificates git gcc > /dev/null\n' |
    docker build -q -t tondo-editor - > /dev/null || return 1
  # With an init: xvfb-run waits for a signal it does not get as process 1.
  docker run --rm --init -u "$me" -e HOME=/tmp -e TONDO_BIN="$PWD/target/debug/tondo" -v "$PWD:$PWD" -w "$PWD/editors/vscode" \
    tondo-editor xvfb-run -a node out/integration/run.js
}

sweep() {
  TONDO_SWEEP=1 cargo test -p tondo --test lsp --test observe --test cli -- the_server_outlives a_damaged_dump the_compiler_outlives
}

# The tests that race, under emulation, again and again, on few cores.
emuloop() {
  set -o pipefail
  . .github/scripts/across.sh aarch64
  CARGO_TARGET_DIR=$S/emu-target cargo test -p tondo --test observe --test mcp --target aarch64-unknown-linux-gnu --no-run --message-format=json |
    jq -r 'select(.profile.test == true and .executable != null) | .executable' > target/emuloop || return 1
  cat > "$S/tmp/emuloop.sh" <<EOF
cd $PWD/crates/tondo
status=0
for round in \$(seq ${EMU_ROUNDS:-5}); do
  for test in $(tr '\n' ' ' < target/emuloop); do
    timeout 1200 \$test > $out/emu-now.txt 2>&1; code=\$?
    echo "round \$round \$(basename \$test): \$code \$(grep -E '^test result' $out/emu-now.txt | tail -1 | cut -c1-60)"
    if [ \$code != 0 ]; then status=1; cp $out/emu-now.txt $out/emu-failed-\$round-\$(basename \$test).txt; grep -E ' FAILED\$' $out/emu-now.txt | cut -c1-120; fi
  done
done
exit \$status
EOF
  image tondo-emu linux/arm64
  docker run --rm ${EMU_INIT---init} --platform linux/arm64 --cpus="${EMU_CPUS:-4}" -u "$me" -e HOME=/tmp -e TMPDIR="$S/tmp" \
    -v "$PWD:$PWD" -v "$S/emu-target:$S/emu-target" -v "$S/tmp:$S/tmp" -v "$out:$out" \
    tondo-emu sh "$S/tmp/emuloop.sh"
}

# The tests that race, on what a runner has, again and again: with nothing
# else to do this machine hides what four cores show.
stress() {
  rounds=${STRESS_ROUNDS:-8}
  image tondo-ci-amd64 linux/amd64
  cargo test -p tondo --test observe --test mcp --test live --test runbooks --no-run 2>&1 | grep -o 'target/debug/deps/[a-z]*-[0-9a-f]*' > target/stress
  status=0
  for round in $(seq "$rounds"); do
    case $((round % 3)) in
      1) cpus=4 ;;
      2) cpus=1 ;;
      0) cpus=0.25 ;;
    esac
    while read -r test; do
      docker run --rm --cpus="$cpus" --memory=16g -u "$me" -e HOME=/tmp -e TMPDIR=/tmp -v "$PWD:$PWD" -w "$PWD/crates/tondo" tondo-ci-amd64 \
        timeout 1800 "$PWD/$test" > "$out/stress-now.txt" 2>&1
      code=$?
      echo "round $round, $cpus cores, $(basename "$test"): $code $(grep -E '^test result' "$out/stress-now.txt" | tail -1 | cut -c1-60)"
      if [ $code != 0 ]; then
        status=1
        cp "$out/stress-now.txt" "$out/stress-failed-$round-$(basename "$test").txt"
        grep -E " FAILED$|panicked at" "$out/stress-now.txt" | cut -c1-200 | head -8
      fi
    done < target/stress
    TONDO_CASES_SCHEDULE=$round cargo test -q --workspace --test cases cases > "$out/stress-now.txt" 2>&1
    code=$?
    echo "round $round, cases in the order of seed $round: $code"
    if [ $code != 0 ]; then
      status=1
      cp "$out/stress-now.txt" "$out/stress-failed-$round-cases.txt"
      grep -E "cases failed|^/" "$out/stress-now.txt" | cut -c1-200 | head -8
    fi
  done
  return $status
}

for stage in "$@"; do
  "$stage" > "$out/$stage.txt" 2>&1
  echo "$stage: $?"
done
echo done
