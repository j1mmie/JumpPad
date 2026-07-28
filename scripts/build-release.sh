#!/usr/bin/env bash
set -euo pipefail

# Builds both release binaries: `xizor` (tiny-skia, software rendering,
# the default) and `xizor-gpu` (wgpu, hardware rendering). They're two
# [[bin]] targets in the same `xizor` package, each requiring the Cargo
# feature that selects its iced compositor backend - see
# `crates/xizor/Cargo.toml`. Cargo names/extensions the output per-target
# automatically (e.g. `xizor.exe` for `x86_64-pc-windows-gnu`), no
# renaming needed here.
#
# Usage:
#   ./scripts/build-release.sh                        # host target
#   ./scripts/build-release.sh x86_64-pc-windows-gnu  # cross-compile for
#                                                      # Windows from Linux/WSL -
#                                                      # requires `rustup target add
#                                                      # x86_64-pc-windows-gnu` and the
#                                                      # mingw-w64 toolchain
#                                                      # (`apt install gcc-mingw-w64`)

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target_args=()
out_dir="target/release"
if [ $# -ge 1 ]; then
    target_args=(--target "$1")
    out_dir="target/$1/release"
fi

cargo build --release "${target_args[@]}" -p xizor --bin xizor
cargo build --release "${target_args[@]}" -p xizor --bin xizor-gpu --no-default-features --features wgpu

echo "done: $out_dir/xizor*, $out_dir/xizor-gpu*"
