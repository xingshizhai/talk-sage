<#
TalkSage v2 构建/运行工具（Windows PowerShell）

用法:
  .\scripts\talksage.ps1 env                 # 环境检查（Rust/Node/模型/静态库）
  .\scripts\talksage.ps1 deps                # 下载依赖（模型 + sherpa 静态库）
  .\scripts\talksage.ps1 build               # 全量编译（Rust + 前端）
  .\scripts\talksage.ps1 dev                 # Tauri 开发模式（热更新）
  .\scripts\talksage.ps1 run                 # 运行桌面 release 版
  .\scripts\talksage.ps1 serve [-host H] [-port P]   # headless 服务（浏览器访问）
  .\scripts\talksage.ps1 listen [-wav F] [-engine E] [-client C] [-save]  # CLI 转写
  .\scripts\talksage.ps1 import [-wav F] [-engine E]   # 导入转写入库
  .\scripts\talksage.ps1 doctor               # 环境诊断（talksage doctor）
  .\scripts\talksage.ps1 test                 # 全量测试（Rust + Vitest）
  .\scripts\talksage.ps1 package              # 打包（NSIS/MSI）
  .\scripts\talksage.ps1 logs                 # 查看最近日志
  .\scripts\talksage.ps1 clean                # 清理构建产物（target/dist/node_modules）

环境变量自动设置（本脚本进程内）:
  CARGO_HOME=$PWD\.cargo-home  SHERPA_ONNX_ARCHIVE_DIR=$PWD\.tools\sherpa-onnx-archives
  TALKSAGE_DATA_DIR=$PWD\.tools\data  TALKSAGE_MODELS_DIR=$PWD\models
代理: 设置 $env:https_proxy / $env:http_proxy 后运行本脚本即可。
#>

param(
    [Parameter(Position = 0)]
    [string]$Command = "help",
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = "Continue"
# PS 7.3+：原生程序 stderr 不触发错误（避免 NativeCommandError 噪音）
$PSNativeCommandUseErrorActionPreference = $false
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# ── 环境变量（项目内隔离，避免污染用户全局） ──────────────────
$env:CARGO_HOME = Join-Path $Root ".cargo-home"
$env:SHERPA_ONNX_ARCHIVE_DIR = Join-Path $Root ".tools\sherpa-onnx-archives"
$env:TALKSAGE_DATA_DIR = Join-Path $Root ".tools\data"
$env:TALKSAGE_MODELS_DIR = Join-Path $Root "models"
$CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $CargoBin) { $env:Path = "$CargoBin;" + $env:Path }

$CliExe = Join-Path $Root "target\debug\talksage.exe"
$ReleaseExe = Join-Path $Root "target\release\talksage-app.exe"

function Write-Step($msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }

# 原生命令包装：显示输出（stderr 并入）并返回退出码
function Invoke-Native([scriptblock]$sb) {
    & $sb 2>&1 | ForEach-Object { Write-Host $_ }
    return $LASTEXITCODE
}

function Invoke-Check($name, $cmd) {
    try {
        $out = & $cmd 2>&1 | Select-Object -First 1
        Write-Host ("  [OK]   {0}: {1}" -f $name, $out) -ForegroundColor Green
        return $true
    } catch {
        Write-Host ("  [FAIL] {0}" -f $name) -ForegroundColor Red
        return $false
    }
}

function Cmd-Env {
    Write-Step "环境检查"
    $null = Invoke-Check "rustc" { rustc --version }
    $null = Invoke-Check "cargo" { cargo --version }
    $null = Invoke-Check "node" { node --version }
    $ok = $true
    $models = @(
        "models\sherpa-onnx-streaming-paraformer-zh",
        "models\sherpa-onnx-streaming-zipformer-en-2023-06-26",
        "models\silero-vad\silero_vad.onnx"
    )
    foreach ($m in $models) {
        $p = Join-Path $Root $m
        if (Test-Path $p) { Write-Host "  [OK]   模型: $m" -ForegroundColor Green }
        else { Write-Host "  [MISS] 模型: $m（运行 .\scripts\talksage.ps1 deps）" -ForegroundColor Yellow; $ok = $false }
    }
    $lib = Join-Path $env:SHERPA_ONNX_ARCHIVE_DIR "sherpa-onnx-*.tar.bz2"
    if (Get-ChildItem $env:SHERPA_ONNX_ARCHIVE_DIR -Filter "sherpa-onnx-*.tar.bz2" -ErrorAction SilentlyContinue) {
        Write-Host "  [OK]   sherpa 静态库已预置" -ForegroundColor Green
    } else {
        Write-Host "  [MISS] sherpa 静态库（构建时自动下载或运行 deps）" -ForegroundColor Yellow
    }
    if (-not $ok) { Write-Host "`n提示: 运行 .\scripts\talksage.ps1 deps 下载缺失依赖" -ForegroundColor Yellow }
}

function Cmd-Deps {
    Write-Step "下载依赖"
    $py = Get-Command python -ErrorAction SilentlyContinue
    if (-not $py) { Write-Host "需要 Python 3（模型下载脚本）"; return 1 }
    # sherpa 静态库（优先已有）
    if (-not (Get-ChildItem $env:SHERPA_ONNX_ARCHIVE_DIR -Filter "sherpa-onnx-*.tar.bz2" -ErrorAction SilentlyContinue)) {
        $null = Invoke-Native { python (Join-Path $PSScriptRoot "download_sherpa.py") }
    } else {
        Write-Host "  sherpa 静态库已存在"
    }
    # 模型
    $null = Invoke-Native { python (Join-Path $PSScriptRoot "download_models.py") all }
    # 前端依赖
    Write-Host "`n安装前端依赖（web/）..."
    Push-Location (Join-Path $Root "web")
    $null = Invoke-Native { npm install --ignore-scripts }
    Pop-Location
    Write-Host "`n依赖就绪。"
}

function Cmd-Build {
    Write-Step "全量编译（cargo + 前端）"
    $code = Invoke-Native { cargo build --workspace }
    if ($code -ne 0) { Write-Host "cargo 编译失败" -ForegroundColor Red; return 1 }
    Write-Host "`n构建前端（web/dist）..."
    Push-Location (Join-Path $Root "web")
    $code2 = Invoke-Native { npm run build }
    Pop-Location
    if ($code2 -ne 0) { Write-Host "前端构建失败" -ForegroundColor Red; return 1 }
    Write-Host "`n编译完成: target\debug\talksage.exe"
}

function Cmd-Dev {
    Write-Step "Tauri 开发模式"
    Push-Location (Join-Path $Root "web")
    $null = Invoke-Native { npx tauri dev }
    Pop-Location
}

function Cmd-Run {
    if (-not (Test-Path $ReleaseExe)) {
        Write-Host "release 未构建，先运行: .\scripts\talksage.ps1 package（或 build 后手动构建 release）" -ForegroundColor Yellow
        Write-Host "快速 release 构建: cd web; npx tauri build --no-bundle"
        return 1
    }
    Write-Step "运行桌面应用（release）"
    Start-Process $ReleaseExe
}

function Cmd-Serve {
    $host = "127.0.0.1"; $port = 8080
    for ($i = 0; $i -lt $Rest.Count; $i++) {
        if ($Rest[$i] -eq "-host" -and $i + 1 -lt $Rest.Count) { $host = $Rest[$i + 1] }
        if ($Rest[$i] -eq "-port" -and $i + 1 -lt $Rest.Count) { $port = $Rest[$i + 1] }
    }
    if (-not (Test-Path (Join-Path $Root "web\dist"))) {
        Write-Host "缺少 web\dist，先运行: .\scripts\talksage.ps1 build" -ForegroundColor Yellow
        return 1
    }
    if (-not (Test-Path $CliExe)) { Write-Host "先运行: .\scripts\talksage.ps1 build" -ForegroundColor Yellow; return 1 }
    Write-Step "headless 服务 http://${host}:${port}"
    $null = Invoke-Native { & $CliExe serve --host $host --port $port }
}

function Cmd-Listen {
    $wav = ""; $engine = "paraformer-zh"; $client = ""; $save = $false
    for ($i = 0; $i -lt $Rest.Count; $i++) {
        if ($Rest[$i] -eq "-wav" -and $i + 1 -lt $Rest.Count) { $wav = $Rest[$i + 1] }
        if ($Rest[$i] -eq "-engine" -and $i + 1 -lt $Rest.Count) { $engine = $Rest[$i + 1] }
        if ($Rest[$i] -eq "-client" -and $i + 1 -lt $Rest.Count) { $client = $Rest[$i + 1] }
        if ($Rest[$i] -eq "-save") { $save = $true }
    }
    if (-not (Test-Path $CliExe)) { Write-Host "先运行: .\scripts\talksage.ps1 build" -ForegroundColor Yellow; return 1 }
    $listenArgs = @("listen", "--input", "mic", "--engine", $engine)
    if ($wav) { $listenArgs[2] = $wav }
    if ($client) { $listenArgs += @("--client", $client) }
    if ($save) { $listenArgs += "--save" }
    Write-Step "CLI 转写: $($listenArgs -join ' ')"
    $null = Invoke-Native { & $CliExe @listenArgs }
}

function Cmd-Import {
    $wav = ""; $engine = "paraformer-zh"
    for ($i = 0; $i -lt $Rest.Count; $i++) {
        if ($Rest[$i] -eq "-wav" -and $i + 1 -lt $Rest.Count) { $wav = $Rest[$i + 1] }
        if ($Rest[$i] -eq "-engine" -and $i + 1 -lt $Rest.Count) { $engine = $Rest[$i + 1] }
    }
    if (-not $wav) { Write-Host "用法: .\scripts\talksage.ps1 import -wav <文件.wav> [-engine paraformer-zh|zipformer-en]"; return 1 }
    if (-not (Test-Path $CliExe)) { Write-Host "先运行: .\scripts\talksage.ps1 build" -ForegroundColor Yellow; return 1 }
    Write-Step "导入转写: $wav"
    $null = Invoke-Native { & $CliExe import $wav --engine $engine }
}

function Cmd-Doctor {
    if (-not (Test-Path $CliExe)) { Write-Host "先运行: .\scripts\talksage.ps1 build" -ForegroundColor Yellow; return 1 }
    $null = Invoke-Native { & $CliExe doctor }
}

function Cmd-Test {
    Write-Step "全量测试"
    $null = Invoke-Native { & (Join-Path $PSScriptRoot "run_tests.ps1") }
}

function Cmd-Package {
    Write-Step "打包（tauri build：NSIS/MSI）"
    Push-Location (Join-Path $Root "web")
    $null = Invoke-Native { npx tauri build }
    Pop-Location
    Write-Host "`n产物:"
    Get-ChildItem (Join-Path $Root "target\release\bundle") -Recurse -File -ErrorAction SilentlyContinue |
        Select-Object @{n='Path';e={$_.FullName.Replace($Root + '\','')}}, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}
}

function Cmd-Logs {
    $logDir = Join-Path $env:TALKSAGE_DATA_DIR "logs"
    if (-not (Test-Path $logDir)) { Write-Host "无日志目录: $logDir"; return }
    $log = Get-ChildItem $logDir -Filter "talksage.log.*" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $log) { Write-Host "无日志文件"; return }
    Write-Host "=== $($log.Name)（最近 50 行）==="
    Get-Content $log.FullName -Tail 50
}

function Cmd-Clean {
    Write-Step "清理构建产物"
    foreach ($d in @("target", "web\dist", "web\node_modules", ".cargo-home", ".tools")) {
        $p = Join-Path $Root $d
        if (Test-Path $p) { Remove-Item -Recurse -Force $p -ErrorAction SilentlyContinue; Write-Host "  已删 $d" }
    }
    Write-Host "清理完成（模型 models/ 保留）。"
}

function Cmd-Help {
    Get-Content $PSCommandPath | Select-String "^#" | ForEach-Object { $_.Line.TrimStart("#", " ") } | Select-Object -First 22
}

# ── 分发 ─────────────────────────────────────────────
switch ($Command.ToLower()) {
    "env"     { Cmd-Env }
    "deps"    { Cmd-Deps }
    "build"   { Cmd-Build }
    "dev"     { Cmd-Dev }
    "run"     { Cmd-Run }
    "serve"   { Cmd-Serve }
    "listen"  { Cmd-Listen }
    "import"  { Cmd-Import }
    "doctor"  { Cmd-Doctor }
    "test"    { Cmd-Test }
    "package" { Cmd-Package }
    "logs"    { Cmd-Logs }
    "clean"   { Cmd-Clean }
    default   { Cmd-Help }
}
