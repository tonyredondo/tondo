#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-log-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.log tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.event.levels = ["Trace", "Debug", "Info", "Warn", "Error", "Fatal"]' testing/stdlib-log.json > "$tmp_dir/fatal.json"
expect_failure fatal-level env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/fatal.json" scripts/stdlib-log-check.sh

jq '.capabilities.forbidden = [.capabilities.forbidden[] | select(. != "global-logger")]' testing/stdlib-log.json > "$tmp_dir/global.json"
expect_failure global-logger env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/global.json" scripts/stdlib-log-check.sh

jq '.capabilities.forbidden = [.capabilities.forbidden[] | select(. != "unbounded-queue")]' testing/stdlib-log.json > "$tmp_dir/unbounded.json"
expect_failure unbounded-queue env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/unbounded.json" scripts/stdlib-log-check.sh

jq '.backpressure.policies = ["Block", "Reject", "Drop", "DropOldest"]' testing/stdlib-log.json > "$tmp_dir/drop-oldest.json"
expect_failure drop-oldest env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/drop-oldest.json" scripts/stdlib-log-check.sh

jq '.formats.closed = ["Text", "JsonLines", "Binary"]' testing/stdlib-log.json > "$tmp_dir/open-format.json"
expect_failure open-format env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/open-format.json" scripts/stdlib-log-check.sh

jq '.surface.signatures[] |= if .id == "logger-emit" then .effect = "selectable" else . end' testing/stdlib-log.json > "$tmp_dir/selectable.json"
expect_failure selectable env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/selectable.json" scripts/stdlib-log-check.sh

jq '.ownership.logger_copyable = true' testing/stdlib-log.json > "$tmp_dir/copy-logger.json"
expect_failure copy-logger env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/copy-logger.json" scripts/stdlib-log-check.sh

jq '.formats.json_lines.final_lf = false' testing/stdlib-log.json > "$tmp_dir/noncanonical-lf.json"
expect_failure noncanonical-lf env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/noncanonical-lf.json" scripts/stdlib-log-check.sh

jq '.values.float_policy = "allow-nan"' testing/stdlib-log.json > "$tmp_dir/nonfinite.json"
expect_failure nonfinite env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/nonfinite.json" scripts/stdlib-log-check.sh

jq '.promotion.next_blocks = ["STD-LOG-001", "DIAG-RUNTIME-001"]' testing/stdlib-log.json > "$tmp_dir/premature-next.json"
expect_failure premature-next env TONDO_STDLIB_LOG_CONTRACT="$tmp_dir/premature-next.json" scripts/stdlib-log-check.sh

for marker in \
    'LogLevel' \
    'Trace' \
    'JsonLines' \
    'tondo-log-event-0.1/1' \
    'Redacted' \
    'Block' \
    'Reject' \
    'Drop' \
    'LogReceipt.Dropped' \
    'Logger.enabled' \
    'Logger.emit' \
    'Logger.flush' \
    'Logger.close' \
    'logger global' \
    'no se publica parcialmente' \
    'DropOldest' \
    'heurística'; do
    grep -Fqi "$marker" docs/contracts/stdlib-log.md \
        || { echo "std.log tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-LOG-001"
  and .event.levels == ["Trace", "Debug", "Info", "Warn", "Error"]
  and .event.fatal_level == "forbidden"
  and .formats.closed == ["Text", "JsonLines"]
  and .formats.json_lines.schema == "tondo-log-event-0.1/1"
  and .backpressure.policies == ["Block", "Reject", "Drop"]
  and .backpressure.unbounded == false
  and .sinks.console.capability == "console"
  and .sinks.file.capability == "filesystem"
  and .sinks.network.capability == "network"
  and .api.no_global_logger == true
  and .surface.selectable_operations == []
  and ([.surface.signatures[] | select(.effect == "selectable")] | length) == 0
  and .ownership.logger_copyable == false
  and .ownership.sink_copyable == false
  and .values.float_policy == "finite-only"
  and .formats.json_lines.final_lf == true
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
  and .implementation.public_api_promoted == false
' testing/stdlib-log.json >/dev/null

echo "std.log tests: OK (events; formats; filters; backpressure; sinks; no hidden control)"
