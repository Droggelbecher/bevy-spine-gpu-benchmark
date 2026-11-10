<#
.SYNOPSIS
  Prints results/results.csv as a comparison table (latest run per backend/scene/count).
.EXAMPLE
  ./summarize.ps1
  ./summarize.ps1 -All
#>
param([switch]$All)

$csv = Join-Path $PSScriptRoot "results/results.csv"
if (-not (Test-Path $csv)) { Write-Host "no results yet ($csv)"; return }

$rows = Import-Csv $csv
if (-not $All) {
    $rows = $rows | Group-Object scene, onscreen, offscreen, backend | ForEach-Object {
        $_.Group | Sort-Object timestamp | Select-Object -Last 1
    }
}

$rows = @($rows | Sort-Object scene, { [int]$_.onscreen }, { [int]$_.offscreen }, backend)

# GPU busy time from run_bench.ps1 -GpuTiming (PresentMon), joined on timestamp + backend.
$gpu = @{}
$gpuCsv = Join-Path $PSScriptRoot "results/gpu_presentmon.csv"
if (Test-Path $gpuCsv) { Import-Csv $gpuCsv | ForEach-Object { $gpu["$($_.timestamp)|$($_.backend)"] = $_ } }
$gpuBusy = @{ Name = 'gpu_busy_ms'; Expression = { $g = $gpu["$($_.timestamp)|$($_.backend)"]; if ($g) { $g.gpu_busy_ms_median } } }

# Bevy version per backend (from the runners' Cargo.lock); spine-gpu is the only one not on 0.19.
$bevyVersion = @{ 'spine-gpu' = '0.14.2'; 'bevy_spine' = '0.19.0'; 'bevy_spine_direct' = '0.19.0'; 'spine2d' = '0.19.0' }
$bevy = @{ Name = 'bevy'; Expression = { $bevyVersion[$_.backend] } }

$rows | Format-Table scene, onscreen, offscreen, backend, $bevy, fps_mean, frame_ms_median, frame_ms_p99, main_ms_mean, render_cpu_ms_mean, gpu_ms_mean, $gpuBusy, vsync_suspect, disturbed -AutoSize

$bad = @($rows | Where-Object { $_.disturbed -eq 'true' })
if ($bad.Count -gt 0) {
    Write-Warning "$($bad.Count) of $($rows.Count) row(s) DISTURBED (window focus/occlusion/resize/spike...); do not compare them:"
    foreach ($r in $bad) {
        Write-Host ("  {0} {1} on={2} off={3} {4}: {5}" -f $r.backend, $r.scene, $r.onscreen, $r.offscreen, $r.timestamp, $r.disturb_reasons) -ForegroundColor Yellow
    }
}
