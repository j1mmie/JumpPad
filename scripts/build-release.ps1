# Builds both release binaries: JumpPad.exe (tiny-skia, software rendering,
# the default) and JumpPadGPU.exe (wgpu, hardware rendering). They're two
# [[bin]] targets in the same `jumppad` package, each requiring the Cargo
# feature that selects its iced compositor backend - see
# crates/jumppad/Cargo.toml. Cargo appends the .exe extension automatically
# on Windows; no renaming needed here.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release -p jumppad --bin JumpPad
cargo build --release -p jumppad --bin JumpPadGPU --no-default-features --features wgpu

Write-Host "done: target/release/JumpPad.exe, target/release/JumpPadGPU.exe"
