#!/usr/bin/env bash

# Wait for one or more crates to appear on crates.io at the current workspace version.
#
# Usage:
#   ./github/scripts/wait_for_crates.sh crate1 [crate2 ...]
#
# Environment variables:
#   MAX_ATTEMPTS  - number of attempts (default: 10)
#   SLEEP_SECONDS - delay between attempts (default: 30)

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 crate1 [crate2 ...]" >&2
  exit 1
fi

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
VERSION=$(
  awk '
    /^\[workspace.package\]/ { in_section=1; next }
    /^\[/ { if (in_section) exit; in_section=0 }
    in_section && $1 ~ /^version/ {
      gsub(/"/, "", $3);
      print $3;
      exit
    }
  ' "${REPO_ROOT}/Cargo.toml"
)

if [[ -z "${VERSION}" ]]; then
  echo "Failed to determine workspace version from Cargo.toml" >&2
  exit 1
fi

MAX_ATTEMPTS=${MAX_ATTEMPTS:-10}
SLEEP_SECONDS=${SLEEP_SECONDS:-30}

for crate in "$@"; do
  echo "Waiting for ${crate} ${VERSION} to appear on crates.io..."
  success=0
  for attempt in $(seq 1 "${MAX_ATTEMPTS}"); do
    if curl --silent --fail "https://crates.io/api/v1/crates/${crate}/${VERSION}" >/dev/null; then
      echo "Found ${crate} ${VERSION} on attempt ${attempt}."
      success=1
      break
    fi
    echo "Attempt ${attempt}/${MAX_ATTEMPTS}: ${crate} ${VERSION} not visible yet; retrying in ${SLEEP_SECONDS}s..."
    sleep "${SLEEP_SECONDS}"
  done
  if [[ "${success}" -ne 1 ]]; then
    echo "Timed out waiting for ${crate} ${VERSION} to propagate to crates.io." >&2
    exit 1
  fi
done
