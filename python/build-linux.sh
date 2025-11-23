#!/usr/bin/env bash
set -euo pipefail

# Build the Linux wheels we already supported (x86_64 and aarch64),
# now using Zig as the linker for both targets.

: "${PYTHON_VERSION:=3.12}"
: "${MATURIN_TARGETS:=x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu}"

export UV_PYTHON_DOWNLOAD_MISSING=1

for target in ${MATURIN_TARGETS}; do
  echo "Building manylinux2014 wheel for ${target} with zig..."
  uv tool run --python "${PYTHON_VERSION}" \
    maturin build \
      --release \
      --target "${target}" \
      --interpreter "python${PYTHON_VERSION}" \
      --compatibility manylinux2014 \
      --zig
done
