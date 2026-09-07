#!/usr/bin/env bash
set -euo pipefail

# Retained command name for existing callers. Synchronization never promotes
# a route merely because its target and corpus exist.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$root/scripts/stdlib-owner-evidence-generate.sh" "$@"
