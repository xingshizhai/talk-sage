# TalkSage v2 全量自动化测试：Rust（单元+集成）+ 前端 Vitest
# 用法: powershell -ExecutionPolicy Bypass -File scripts\run_tests.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "===== [1/2] Rust: cargo test --workspace =====" -ForegroundColor Cyan
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
$env:CARGO_HOME = "$root\.cargo-home"
if (Test-Path "$root\.tools\sherpa-onnx-archives") {
    $env:SHERPA_ONNX_ARCHIVE_DIR = "$root\.tools\sherpa-onnx-archives"
}
cargo test --workspace
if ($LASTEXITCODE -ne 0) { Write-Host "Rust 测试失败" -ForegroundColor Red; exit 1 }

Write-Host "`n===== [2/2] Frontend: vitest run =====" -ForegroundColor Cyan
Set-Location "$root\web"
npx vitest run
if ($LASTEXITCODE -ne 0) { Write-Host "前端测试失败" -ForegroundColor Red; exit 1 }

Write-Host "`n✅ 全部测试通过" -ForegroundColor Green
