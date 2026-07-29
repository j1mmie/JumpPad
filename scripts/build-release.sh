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

# Branched rather than built up as an array: an empty array expanded under
# `set -u` (`"${target_args[@]}"`) throws "unbound variable" on bash 3.2,
# which is what `/usr/bin/env bash` resolves to by default on macOS.
if [ $# -ge 1 ]; then
    cargo build --release --target "$1" -p xizor --bin xizor
    cargo build --release --target "$1" -p xizor --bin xizor-gpu --no-default-features --features wgpu
    out_dir="target/$1/release"
else
    cargo build --release -p xizor --bin xizor
    cargo build --release -p xizor --bin xizor-gpu --no-default-features --features wgpu
    out_dir="target/release"
fi

echo "done: $out_dir/xizor*, $out_dir/xizor-gpu*"
