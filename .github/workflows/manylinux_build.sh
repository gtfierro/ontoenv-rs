# from https://github.com/oxigraph/oxigraph/blob/main/.github/workflows/manylinux_build.sh
set -euxo pipefail
cd /workdir
if command -v dnf >/dev/null 2>&1; then
  dnf -y update
  dnf -y install clang cmake make gcc-c++
elif command -v yum >/dev/null 2>&1; then
  yum -y update
  yum -y install clang cmake make gcc-c++
else
  echo "No supported package manager found (dnf/yum)" >&2
  exit 1
fi
curl https://static.rust-lang.org/rustup/dist/%arch%-unknown-linux-gnu/rustup-init --output rustup-init
chmod +x rustup-init
./rustup-init -y --profile minimal --default-toolchain stable
source "$HOME/.cargo/env"
export PATH="${PATH}:/opt/python/cp37-cp37m/bin:/opt/python/cp38-cp38/bin:/opt/python/cp39-cp39/bin:/opt/python/cp310-cp310/bin:/opt/python/cp311-cp311/bin:/opt/python/cp312-cp312/bin"
cd python
python3.12 -m venv venv
source venv/bin/activate
pip install -r requirements.dev.txt
maturin develop --release --features "abi3 cli"
maturin build --release --features "abi3 cli" --compatibility manylinux_2_28
if [ %for_each_version% ]; then
  for VERSION in 8 9 10 11 12; do
    maturin build --release --features "abi3 cli" --interpreter "python3.$VERSION" --compatibility manylinux_2_28
  done
  for VERSION in 11; do
    maturin build --release --features "abi3 cli" --interpreter "pypy3.$VERSION" --compatibility manylinux_2_28
  done
fi
