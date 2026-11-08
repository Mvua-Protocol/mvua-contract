#!/usr/bin/env bash
# check-specs.sh - fail if any contract spec references an undefined type.
#
# Builds every deployable contract (each contracts/* crate with a cdylib
# target) to wasm32v1-none release, as `stellar contract build` does, then runs
# the host side `spec-check` tool over the resulting wasm. Catches a type alias
# leaking into a #[contracttype] / #[contractevent] / public fn signature as a
# dangling user-defined type ("Missing Entry Bps", DR-0032), which the in host
# tests cannot see because they never round trip through the spec.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

packages=()
for manifest in contracts/*/Cargo.toml; do
  if grep -q 'crate-type *= *\[[^]]*"cdylib"' "${manifest}"; then
    packages+=("$(sed -n 's/^name *= *"\(.*\)"/\1/p' "${manifest}" | head -n 1)")
  fi
done

if [ "${#packages[@]}" -eq 0 ]; then
  echo "error: no cdylib contract crates found under contracts/" >&2
  exit 1
fi

build_args=()
wasms=()
for p in "${packages[@]}"; do
  build_args+=(-p "${p}")
  wasms+=("target/wasm32v1-none/release/${p//-/_}.wasm")
done

echo "Building ${packages[*]} for wasm32v1-none"
cargo build --release --target wasm32v1-none "${build_args[@]}"

cargo run --quiet -p spec-check -- "${wasms[@]}"
