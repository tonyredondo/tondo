#!/usr/bin/env bash
# Hosted CI has a system manager and passwordless sudo, but no user manager.
# Run checks as the original runner user in a transient delegated service.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: ci-test-scope.sh <check-command> [arguments...]" >&2
  exit 2
fi
if [ "$(uname -s)" != Linux ]; then
  exec "$@"
fi
if [ "${GITHUB_ACTIONS:-}" != true ]; then
  echo "ci-test-scope.sh is for hosted CI; use a delegated user scope locally" >&2
  exit 3
fi

# Forward only the existing build/check inputs, never the ambient credential
# environment. systemd supplies HOME and LOGNAME for the selected runner user.
tondo_check_environment=("PATH=$PATH")
for tondo_variable in CARGO_HOME RUSTUP_HOME CARGO_BUILD_JOBS CARGO_TARGET_DIR \
  CARGO_INCREMENTAL TONDO_TEST_TARGET TONDO_TEST_SEED TONDO_FAST_BASE \
  TONDO_LLVM_LLC TONDO_NATIVE_CC GITHUB_ACTIONS GITHUB_BASE_REF RUNNER_OS; do
  if [[ -v "$tondo_variable" ]]; then
    tondo_check_environment+=("$tondo_variable=${!tondo_variable}")
  fi
done
exec sudo --non-interactive systemd-run --quiet --wait --pipe --collect \
  --service-type=exec --expand-environment=no \
  --uid="$(id -u)" --gid="$(id -g)" --property=Delegate=yes \
  --property=KillMode=control-group --working-directory="$PWD" \
  /usr/bin/env "${tondo_check_environment[@]}" \
  bash scripts/test-process-scope.sh "$@"
