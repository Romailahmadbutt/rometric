# Rometric — native Windows build helper
# Requires: Rust stable + Visual Studio C++ Build Tools.

param(
    [switch]$Cross
)

$ErrorActionPreference = "Stop"

if ($Cross) {
    rustup target add x86_64-pc-windows-msvc
    cargo xwin build --release --target x86_64-pc-windows-msvc
    Write-Host "Built: target\x86_64-pc-windows-msvc\release\rometric.exe"
} else {
    cargo build --release
    Write-Host "Built: target\release\rometric.exe"
}
