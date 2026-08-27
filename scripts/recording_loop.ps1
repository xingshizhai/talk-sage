# recording_loop.ps1 — 录音测试闭环：裁剪静音 → 回放验证转写。
#
# 目标：把真实会议录音变成可复现的测试素材（边用边录 → 裁剪 → 回归验证）。
#
# 用法:
#   .\scripts\recording_loop.ps1 [-RecDir <目录>] [-ModelsDir <目录>] [-Latest <N>] [-NoAsr]
#
# 默认行为:
#   - RecDir   = $env:TALKSAGE_DATA_DIR\recordings 或 ~/.talksage/recordings
#   - ModelsDir = .\models（含 silero-vad + paraformer-zh）
#   - 对每个 wav: talksage trim（去掉静音）→ talksage listen --input（真实 ASR 回放）
#   - 汇总表: 原始时长 / 裁剪后时长 / 压缩率

param(
    [string]$RecDir = "",
    [string]$ModelsDir = "models",
    [int]$Latest = 0,          # 只处理最近 N 个文件（0 = 全部）
    [switch]$NoAsr             # 跳过回放验证（只裁剪）
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false

$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# 定位 talksage.exe（release 优先）。产物可能在 C:\wt（脚本默认 CARGO_TARGET_DIR）、
# 外部 CARGO_TARGET_DIR 或项目 target\ 下，按优先级搜索。
$exe = ""
foreach ($d in @($env:CARGO_TARGET_DIR, "C:\wt", (Join-Path $repo "target"))) {
    if (-not $d) { continue }
    foreach ($profile in @("release", "debug")) {
        $p = Join-Path $d "$profile\talksage.exe"
        if (Test-Path $p) { $exe = $p; break }
    }
    if ($exe) { break }
}
if (-not $exe) {
    Write-Error "未找到 talksage.exe，请先构建: scripts\talksage.ps1 build"
    exit 1
}

# 录音目录
if (-not $RecDir) {
    if ($env:TALKSAGE_DATA_DIR) { $RecDir = Join-Path $env:TALKSAGE_DATA_DIR "recordings" }
    else { $RecDir = Join-Path $HOME ".talksage\recordings" }
}
if (-not (Test-Path $RecDir)) {
    Write-Error "录音目录不存在: $RecDir（先运行应用开始监听，或 talksage record 录制）"
    exit 1
}
if (-not (Test-Path (Join-Path $ModelsDir "silero-vad\silero_vad.onnx"))) {
    Write-Error "模型不完整: $ModelsDir（请先运行 scripts\download_models.py 或设 -ModelsDir）"
    exit 1
}
$env:TALKSAGE_MODELS_DIR = (Resolve-Path $ModelsDir).Path

$wavs = Get-ChildItem -Path $RecDir -Filter *.wav |
    Where-Object { $_.Name -notlike "*.trimmed.wav" } |
    Sort-Object LastWriteTime -Descending
if ($Latest -gt 0) { $wavs = $wavs | Select-Object -First $Latest }
if ($wavs.Count -eq 0) {
    Write-Host "录音目录无 wav 文件: $RecDir"
    exit 0
}

Write-Host "== 录音测试闭环 =="
Write-Host "录音目录 : $RecDir"
Write-Host "处理文件 : $($wavs.Count) 个（latest=$Latest）"
Write-Host ""

$rows = @()
foreach ($w in $wavs) {
    $trimmed = [System.IO.Path]::ChangeExtension($w.FullName, ".trimmed.wav")
    $origBytes = $w.Length
    Write-Host "== [$($w.Name)] 静音裁剪 =="
    & $exe trim $w.FullName -o $trimmed
    if ($LASTEXITCODE -ne 0) { Write-Host "  裁剪失败，跳过"; continue }

    $trimBytes = if (Test-Path $trimmed) { (Get-Item $trimmed).Length } else { 0 }
    $sizeRatio = if ($origBytes -gt 0) { [math]::Round($trimBytes / $origBytes * 100, 0) } else { 100 }

    $row = [pscustomobject]@{
        文件   = $w.Name
        原始KB = [math]::Round($origBytes / 1KB, 1)
        裁剪KB = [math]::Round($trimBytes / 1KB, 1)
        压缩率 = "$sizeRatio%"
        转写   = "—"
    }

    if (-not $NoAsr -and (Test-Path $trimmed) -and $trimBytes -gt 44) {
        Write-Host "-- [$($w.Name)] 回放验证（真实 ASR）--"
        $asrOut = & $exe listen --input $trimmed 2>&1 | Out-String
        # 提取 final 段（"[label] text" 行；partial 行以 ▍ 结尾）
        $texts = $asrOut -split "`r?`n" |
            Where-Object { $_ -match '^\[[^\]]+\] .+' -and $_ -notmatch '▍$' } |
            ForEach-Object { ($_ -replace '^\[[^\]]+\]\s*', '').Trim() } |
            Where-Object { $_ -ne "" }
        if ($texts.Count -gt 0) {
            $joined = ($texts -join " / ")
            if ($joined.Length -gt 60) { $joined = $joined.Substring(0, 60) + "…" }
            $row.转写 = $joined
        } else {
            $row.转写 = "（无语音）"
        }
    }
    $rows += $row
    Write-Host ""
}

Write-Host "== 汇总 =="
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
$sum = $rows | Measure-Object -Property 裁剪KB -Sum
Write-Host "完成：$($rows.Count) 个文件。裁剪后的 .trimmed.wav 可直接作为回归测试素材（跑 scripts\run_tests.ps1 覆盖 ASR 链路）。"
