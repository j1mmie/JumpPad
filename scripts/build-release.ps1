# Builds both release binaries: xizor.exe (tiny-skia, software rendering,
# the default) and xizor-gpu.exe (wgpu, hardware rendering). They're two
# [[bin]] targets in the same `xizor` package, each requiring the Cargo
# feature that selects its iced compositor backend - see
# crates/xizor/Cargo.toml. Cargo appends the .exe extension automatically
# on Windows; no renaming needed here.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release -p xizor --bin xizor
cargo build --release -p xizor --bin xizor-gpu --no-default-features --features wgpu

Write-Host "done: target/release/xizor.exe, target/release/xizor-gpu.exe"
