# platform: any
param(
    [string]$CargoTargetRoot = "",
    [string]$PerfReportOutputPath = "tmp/layer-perf-smoke.txt",
    [switch]$SkipPerfReport
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$userHome = if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
    $env:USERPROFILE
} elseif (-not [string]::IsNullOrWhiteSpace($env:HOME)) {
    $env:HOME
} else {
    (Resolve-Path "~").Path
}
$pathSeparator = if ($env:OS -eq "Windows_NT") { ";" } else { ":" }

$cargoBin = Join-Path $userHome ".cargo/bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin$pathSeparator$env:Path"
}
if ([string]::IsNullOrWhiteSpace($CargoTargetRoot)) {
    $CargoTargetRoot = Join-Path $userHome ".cargo-target/gitgraph-lab"
}
if (-not (Test-Path $CargoTargetRoot)) {
    New-Item -Path $CargoTargetRoot -ItemType Directory -Force | Out-Null
}
$env:CARGO_TARGET_DIR = $CargoTargetRoot

function Invoke-LayerLane {
    param(
        [string]$Name,
        [string]$Command,
        [string[]]$Arguments
    )

    Write-Host ("Running layer lane: " + $Name)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process `
        -FilePath $Command `
        -ArgumentList $Arguments `
        -NoNewWindow `
        -Wait `
        -PassThru
    $code = $process.ExitCode
    $sw.Stop()
    if ($code -ne 0) {
        throw "Layer lane failed: $Name"
    }

    return [pscustomobject]@{
        lane    = $Name
        seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 3)
    }
}

function Get-ReportValue {
    param(
        [string]$Path,
        [string]$Key
    )

    if (-not (Test-Path $Path)) {
        return "n/a"
    }

    $line = Select-String -Path $Path -Pattern ("^\s*" + [Regex]::Escape($Key) + ":\s*(.+)$") | Select-Object -First 1
    if ($null -eq $line) {
        return "n/a"
    }
    $value = $line.Matches[0].Groups[1].Value.Trim()
    if ([string]::IsNullOrWhiteSpace($value)) {
        return "n/a"
    }
    return $value
}

Push-Location $repoRoot
try {
    if (-not $SkipPerfReport) {
        $perfOutputDir = Split-Path -Parent $PerfReportOutputPath
        if (-not [string]::IsNullOrWhiteSpace($perfOutputDir)) {
            New-Item -ItemType Directory -Path $perfOutputDir -Force | Out-Null
        }
        & "$PSScriptRoot\perf-report.ps1" `
            -CargoTargetRoot $CargoTargetRoot `
            -Cycles 80 `
            -FeatureCommits 4 `
            -IncrementalCommits 20 `
            -QueryRuns 10 `
            -WindowLimit 200 `
            -MaxNodes 2000 `
            -OutputPath $PerfReportOutputPath
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to run perf-report smoke lane."
        }
    }

    $lanes = @()
    $lanes += Invoke-LayerLane -Name "prebuild-tests" -Command "cargo" -Arguments @(
        "test", "--package", "gitgraph-lab-core", "--locked", "--no-run"
    )
    $lanes += Invoke-LayerLane -Name "adapters-unit" -Command "cargo" -Arguments @(
        "test", "--package", "gitgraph-lab-core", "--locked", "--lib", "adapters::", "--", "--nocapture"
    )
    $lanes += Invoke-LayerLane -Name "application-unit" -Command "cargo" -Arguments @(
        "test", "--package", "gitgraph-lab-core", "--locked", "--lib", "application::", "--", "--nocapture"
    )
    $lanes += Invoke-LayerLane -Name "domain-unit" -Command "cargo" -Arguments @(
        "test", "--package", "gitgraph-lab-core", "--locked", "--lib", "domain::", "--", "--nocapture"
    )
    $lanes += Invoke-LayerLane -Name "support-unit" -Command "cargo" -Arguments @(
        "test", "--package", "gitgraph-lab-core", "--locked", "--lib", "support::", "--", "--nocapture"
    )
    $lanes += Invoke-LayerLane -Name "integration-smoke" -Command "cargo" -Arguments @(
        "test", "--package", "gitgraph-lab-core", "--locked", "--test", "indexer_smoke", "--", "--nocapture"
    )

    $phaseRows = @(
        [pscustomobject]@{ metric = "fixture_generation_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "fixture_generation" }
        [pscustomobject]@{ metric = "full_index_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "full_index" }
        [pscustomobject]@{ metric = "incremental_index_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "incremental_index" }
        [pscustomobject]@{ metric = "query_benchmark_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "query_benchmark" }
        [pscustomobject]@{ metric = "in_memory_graph_hydration_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "in_memory_graph_hydration" }
        [pscustomobject]@{ metric = "in_memory_query_benchmark_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "in_memory_query_benchmark" }
        [pscustomobject]@{ metric = "ingest_commit_walk_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "ingest_commit_walk_ms" }
        [pscustomobject]@{ metric = "ingest_file_diff_extract_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "ingest_file_diff_extract_ms" }
        [pscustomobject]@{ metric = "ingest_file_change_upsert_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "ingest_file_change_upsert_ms" }
        [pscustomobject]@{ metric = "ingest_tx_commit_ms"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "ingest_tx_commit_ms" }
        [pscustomobject]@{ metric = "peak_observed_rss_bytes"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "peak_observed_rss" }
        [pscustomobject]@{ metric = "final_rss_bytes"; value = Get-ReportValue -Path $PerfReportOutputPath -Key "final_rss" }
    )

    Write-Host ""
    Write-Host "Layer lane durations (seconds):"
    $lanes | Format-Table -AutoSize

    Write-Host ""
    Write-Host "Phase metrics from perf-report smoke:"
    $phaseRows | Format-Table -AutoSize

    $summaryPath = "tmp/layer-test-matrix.txt"
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("=== Layer Test Matrix ===")
    $lines.Add("layer_lane_seconds:")
    foreach ($lane in $lanes) {
        $lines.Add(("  {0}: {1}" -f $lane.lane, $lane.seconds))
    }
    $lines.Add("phase_metrics:")
    foreach ($row in $phaseRows) {
        $lines.Add(("  {0}: {1}" -f $row.metric, $row.value))
    }
    Set-Content -Path $summaryPath -Value $lines
    Write-Host ""
    Write-Host ("Layer matrix summary written: " + $summaryPath)
}
finally {
    Pop-Location
}
