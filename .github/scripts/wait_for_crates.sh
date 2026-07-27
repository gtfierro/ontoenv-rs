#!/usr/bin/env bash

# Wait for one or more crates to appear on crates.io at the current workspace version.
#
# Usage:
#   ./github/scripts/wait_for_crates.sh crate1 [crate2 ...]
#
# Environment variables:
#   MAX_ATTEMPTS  - number of attempts (default: 10)
#   SLEEP_SECONDS - delay between attempts (default: 30)
#
# crates.io rejects requests that do not carry a descriptive User-Agent (see
# https://crates.io/data-access), so one is always sent. Only a 404 means "not
# published yet" and is worth retrying; any other unexpected status is a hard
# error rather than something to spend the whole retry budget on.

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
USER_AGENT="ontoenv-rs release CI (+https://github.com/gtfierro/ontoenv-rs)"

# Echoes the HTTP status for a crate version, or "000" if the request itself
# failed (DNS, TLS, connection reset).
crate_status() {
  curl --silent --output /dev/null --write-out '%{http_code}' \
    --user-agent "${USER_AGENT}" \
    "https://crates.io/api/v1/crates/$1/${VERSION}" || echo "000"
}

for crate in "$@"; do
  echo "Waiting for ${crate} ${VERSION} to appear on crates.io..."
  success=0
  for attempt in $(seq 1 "${MAX_ATTEMPTS}"); do
    status=$(crate_status "${crate}")
    case "${status}" in
      200)
        echo "Found ${crate} ${VERSION} on attempt ${attempt}."
        success=1
        break
        ;;
      404|000)
        echo "Attempt ${attempt}/${MAX_ATTEMPTS}: ${crate} ${VERSION} not visible yet (HTTP ${status}); retrying in ${SLEEP_SECONDS}s..."
        ;;
      *)
        echo "crates.io returned HTTP ${status} for ${crate} ${VERSION}; refusing to treat this as a propagation delay." >&2
        exit 1
        ;;
    esac
    sleep "${SLEEP_SECONDS}"
  done
  if [[ "${success}" -ne 1 ]]; then
    echo "Timed out waiting for ${crate} ${VERSION} to propagate to crates.io." >&2
    exit 1
  fi
done
