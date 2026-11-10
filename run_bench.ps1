<#
.SYNOPSIS
  Sweeps scenes x backends x instance counts. Results are appended to results/results.csv.
.DESCRIPTION
  -GpuTiming records every run with PresentMon (console app, works unelevated) and appends the GPU busy time of the
  benchmark process to results/gpu_presentmon.csv, keyed by the results.csv row's timestamp + backend
  (summarize.ps1 joins them). Same measurement for every backend; the runners are not involved.
.EXAMPLE
  ./run_bench.ps1
  ./run_bench.ps1 -Backends spine-gpu,spine2d -Scenes ella_random -Counts 500,2000,8000 -Duration 10
  ./run_bench.ps1 -GpuTiming
#>
param(
    [string[]]$Backends = @("spine-gpu", "bevy-spine", "bevy-spine-direct", "spine2d"),
    # mika_* scenes are excluded: spine-gpu does not support rotated atlas regions (deferred).
    [string[]]$Scenes = @("ella_same", "ella_same_phase", "ella_random", "ella_random_offscreen",
        "ella_random_ysort", "ella_random_bob", "ella_random_ysort_bob", "mixed3_random_bob", "mixed3_random_ysort_bob"),
    # Onscreen instance counts; empty = use the scene file's value. Offscreen count always comes from the scene file.
    [int[]]$Counts = @(),
    [double]$Warmup = 3,
    [double]$Duration = 15,
    [switch]$NoScreenshot,
    [switch]$SkipBuild,
    # Record GPU busy time with PresentMon (see .DESCRIPTION).
    [switch]$GpuTiming,
    [string]$PresentMon = "C:\Program Files\Intel\PresentMon\PresentMonConsoleApplication\PresentMon-2.6.0-x64.exe"
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$resultsCsv = "$root/results/results.csv"
$gpuCsv = "$root/results/gpu_presentmon.csv"
$pmDir = "$root/results/presentmon"
$pmSession = "bench_presentmon"

if ($GpuTiming) {
    if (-not (Test-Path $PresentMon)) { throw "PresentMon not found: $PresentMon (pass -PresentMon <path>)" }
    New-Item -ItemType Directory -Force $pmDir | Out-Null
    if (-not (Test-Path $gpuCsv)) {
        "timestamp,backend,scene,onscreen,offscreen,pm_frames,gpu_busy_ms_median,gpu_busy_ms_p95,gpu_time_ms_median,pm_frame_ms_median,pm_present_mode" |
            Out-File -Encoding utf8 $gpuCsv
    }
    # A session left over from an aborted sweep would make the next start fail.
    & $PresentMon --session_name $pmSession --terminate_existing_session 2>&1 | Out-Null
}

function Get-RowCount {
    if (Test-Path $resultsCsv) { @(Get-Content $resultsCsv).Count } else { 0 }
}

function Get-Quantile([double[]]$v, [double]$q) {
    if ($v.Count -eq 0) { return $null }
    $s = $v | Sort-Object
    $s[[math]::Min($s.Count - 1, [math]::Floor($q * $s.Count))]
}

# Reduce one PresentMon capture to the benchmark process's measured frames and append a row to gpu_presentmon.csv.
function Write-GpuRow($pmFile, $exeName, $row) {
    $frames = @(Import-Csv $pmFile | Where-Object { $_.Application -eq $exeName })
    if ($frames.Count -eq 0) { Write-Warning "PresentMon: no frames of $exeName in $pmFile"; return }
    # The run ends with measure -> screenshot -> exit, so take the last Duration seconds minus margins: drop the
    # final 0.5 s (screenshot, shutdown) and 0.5 s at the start (warmup -> measure transition).
    $tEnd = [double]$frames[-1].TimeInMs - 500
    $tStart = $tEnd - ($Duration - 1) * 1000
    $win = @($frames | Where-Object { $t = [double]$_.TimeInMs; $t -ge $tStart -and $t -le $tEnd })
    if ($win.Count -lt 10) { Write-Warning "PresentMon: only $($win.Count) frames in the measure window" }
    $num = { param($col) [double[]]@($win | Where-Object { $_.$col -ne 'NA' -and $_.$col -ne '' } | ForEach-Object { [double]$_.$col }) }
    $busy = & $num 'MsGPUBusy'
    $mode = ($win | Group-Object PresentMode | Sort-Object Count -Descending | Select-Object -First 1).Name
    # Invariant culture: '-f' would use the OS locale's decimal comma.
    $fmt = { param($x) if ($null -eq $x) { '' } else { [string]::Format([cultureinfo]::InvariantCulture, '{0:0.####}', $x) } }
    $line = @($row.timestamp, $row.backend, $row.scene, $row.onscreen, $row.offscreen, $win.Count,
        (& $fmt (Get-Quantile $busy 0.5)), (& $fmt (Get-Quantile $busy 0.95)),
        (& $fmt (Get-Quantile (& $num 'MsGPUTime') 0.5)), (& $fmt (Get-Quantile (& $num 'MsBetweenPresents') 0.5)),
        $mode) -join ','
    $line | Out-File -Append -Encoding utf8 $gpuCsv
    # Keep only the benchmark process's rows of the raw capture (the full one has every process on the machine).
    $frames | Export-Csv -NoTypeInformation -Encoding utf8 ($pmFile -replace '\.csv$', '.bench.csv')
    Remove-Item $pmFile
    Write-Host "   PresentMon: $($win.Count) frames, GPU busy median $(& $fmt (Get-Quantile $busy 0.5)) ms"
}

if (-not $SkipBuild) {
    foreach ($b in $Backends) {
        Write-Host "== building $b"
        # Run cargo from inside the runner dir so its rust-toolchain.toml applies (--manifest-path doesn't pick it up).
        Push-Location "$root/runners/$b"
        cargo build --release
        $code = $LASTEXITCODE
        Pop-Location
        if ($code -ne 0) { throw "build failed: $b" }
    }
}

$countList = if ($Counts.Count -gt 0) { $Counts } else { @($null) }

foreach ($scene in $Scenes) {
    foreach ($count in $countList) {
        foreach ($b in $Backends) {
            $cmdArgs = @("run", "--release", "--",
                "--scene", "$root/scenes/$scene.toml", "--warmup", $Warmup, "--duration", $Duration)
            if ($null -ne $count) { $cmdArgs += @("--onscreen", $count) }
            if ($NoScreenshot) { $cmdArgs += "--no-screenshot" }
            Write-Host "== $b / $scene / onscreen=$(if ($null -ne $count) { $count } else { 'scene default' })"
            $rowsBefore = Get-RowCount
            $pmProc = $null
            if ($GpuTiming) {
                $pmFile = "$pmDir/$((Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ'))_${b}_${scene}_$count.csv"
                $pmProc = Start-Process -FilePath $PresentMon -PassThru -WindowStyle Hidden -ArgumentList @(
                    "--session_name", $pmSession, "--no_console_stats", "--output_file", "`"$pmFile`"")
                Start-Sleep -Milliseconds 1500  # let the trace session start
            }
            Push-Location "$root/runners/$b"
            try {
                & cargo @cmdArgs
                $code = $LASTEXITCODE
            } finally {
                Pop-Location
                if ($pmProc) {
                    & $PresentMon --session_name $pmSession --terminate_existing_session 2>&1 | Out-Null
                    if (-not $pmProc.WaitForExit(30000)) { $pmProc.Kill() }
                }
            }
            if ($code -ne 0) { Write-Warning "run failed: $b $scene $count" }
            if ($pmProc) {
                if ($code -eq 0 -and (Get-RowCount) -gt $rowsBefore) {
                    $row = (Import-Csv $resultsCsv)[-1]
                    $exeName = (Select-String -Path "$root/runners/$b/Cargo.toml" -Pattern '^name\s*=\s*"([^"]+)"' |
                        Select-Object -First 1).Matches[0].Groups[1].Value + ".exe"
                    Write-GpuRow $pmFile $exeName $row
                } else {
                    Remove-Item $pmFile -ErrorAction SilentlyContinue
                }
            }
        }
    }
}

Write-Host "`n== summary"
& "$root/summarize.ps1"
