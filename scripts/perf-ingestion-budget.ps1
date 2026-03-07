# platform: any
param(
    [string]$CargoTargetRoot = "",
    [ValidateSet("25k", "90k")]
    [string]$Profile = "25k",
    [string]$OutputPath = "",
    [double]$MaxFullIndexMs = 45000,
    [double]$MaxIncrementalIndexMs = 15000,
    [UInt64]$MaxPeakRssBytes = 60000000,
    [switch]$PreconditionCommitGraph,
    [switch]$ProbeOnly
)

$ErrorActionPreference = "Stop"

function Get-ReportValue {
    param(
        [string]$Path,
        [string]$Key
    )

    if (-not (Test-Path $Path)) {
        throw "Missing report file: $Path"
    }

    $line = Select-String -Path $Path -Pattern ("^\s*" + [Regex]::Escape($Key) + ":\s*(.+)$") | Select-Object -First 1
    if ($null -eq $line) {
        throw "Missing key in report: $Key"
    }

    return $line.Matches[0].Groups[1].Value.Trim()
}

function Parse-DoubleOrThrow {
    param(
        [string]$Raw,
        [string]$Field
    )

    $value = 0.0
    if (-not [double]::TryParse($Raw, [System.Globalization.NumberStyles]::Float, [System.Globalization.CultureInfo]::InvariantCulture, [ref]$value)) {
        throw "Failed to parse numeric value for ${Field}: $Raw"
    }
    return $value
}

if ($ProbeOnly) {
    $global:LASTEXITCODE = 0
    & "$PSScriptRoot\perf-report.ps1" -ProbeOnly -CargoTargetRoot $CargoTargetRoot
    if (-not $?) {
        throw "Ingestion budget probe failed."
    }
    Write-Host "Ingestion budget probe passed."
    return
}

if ($Profile -eq "90k") {
    $defaultOutputPath = "tmp/perf-report-ingestion-budget-90k.txt"
    $cycles = 9000
    $featureCommits = 6
    $incrementalCommits = 9000
    $queryRuns = 20
    $windowLimit = 1000
    $maxNodes = 90000
    $defaultMaxFullIndexMs = 180000
    $defaultMaxIncrementalIndexMs = 90000
    $defaultMaxPeakRssBytes = [UInt64]200000000
} else {
    $defaultOutputPath = "tmp/perf-report-ingestion-budget.txt"
    $cycles = 2500
    $featureCommits = 6
    $incrementalCommits = 2500
    $queryRuns = 20
    $windowLimit = 500
    $maxNodes = 25000
    $defaultMaxFullIndexMs = 45000
    $defaultMaxIncrementalIndexMs = 15000
    $defaultMaxPeakRssBytes = [UInt64]60000000
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = $defaultOutputPath
}
if (-not $PSBoundParameters.ContainsKey("MaxFullIndexMs")) {
    $MaxFullIndexMs = $defaultMaxFullIndexMs
}
if (-not $PSBoundParameters.ContainsKey("MaxIncrementalIndexMs")) {
    $MaxIncrementalIndexMs = $defaultMaxIncrementalIndexMs
}
if (-not $PSBoundParameters.ContainsKey("MaxPeakRssBytes")) {
    $MaxPeakRssBytes = $defaultMaxPeakRssBytes
}

Write-Host ("Ingestion budget profile: {0}" -f $Profile)
Write-Host (
    "Workload: cycles={0} feature_commits={1} incremental_commits={2} query_runs={3} window_limit={4} max_nodes={5}" `
        -f $cycles, $featureCommits, $incrementalCommits, $queryRuns, $windowLimit, $maxNodes
)
Write-Host (
    "Thresholds: max_full_index_ms={0} max_incremental_index_ms={1} max_peak_rss_bytes={2}" `
        -f $MaxFullIndexMs, $MaxIncrementalIndexMs, $MaxPeakRssBytes
)

& "$PSScriptRoot\perf-report.ps1" `
    -CargoTargetRoot $CargoTargetRoot `
    -Cycles $cycles `
    -FeatureCommits $featureCommits `
    -IncrementalCommits $incrementalCommits `
    -QueryRuns $queryRuns `
    -WindowLimit $windowLimit `
    -MaxNodes $maxNodes `
    -PreconditionCommitGraph:$PreconditionCommitGraph `
    -OutputPath $OutputPath
if ($LASTEXITCODE -ne 0) {
    throw "perf-report execution failed for ingestion budget lane."
}

$fullIndexRaw = Get-ReportValue -Path $OutputPath -Key "full_index"
$incrementalIndexRaw = Get-ReportValue -Path $OutputPath -Key "incremental_index"
$peakRssRaw = Get-ReportValue -Path $OutputPath -Key "peak_observed_rss"
$ingestCommitWalkRaw = Get-ReportValue -Path $OutputPath -Key "ingest_commit_walk_ms"
$ingestDiffRaw = Get-ReportValue -Path $OutputPath -Key "ingest_file_diff_extract_ms"
$ingestUpsertRaw = Get-ReportValue -Path $OutputPath -Key "ingest_file_change_upsert_ms"
$ingestTxCommitRaw = Get-ReportValue -Path $OutputPath -Key "ingest_tx_commit_ms"

$fullIndexMs = Parse-DoubleOrThrow -Raw $fullIndexRaw -Field "full_index"
$incrementalIndexMs = Parse-DoubleOrThrow -Raw $incrementalIndexRaw -Field "incremental_index"
$peakRssBytes = [UInt64](Parse-DoubleOrThrow -Raw $peakRssRaw -Field "peak_observed_rss")
[void](Parse-DoubleOrThrow -Raw $ingestCommitWalkRaw -Field "ingest_commit_walk_ms")
[void](Parse-DoubleOrThrow -Raw $ingestDiffRaw -Field "ingest_file_diff_extract_ms")
[void](Parse-DoubleOrThrow -Raw $ingestUpsertRaw -Field "ingest_file_change_upsert_ms")
[void](Parse-DoubleOrThrow -Raw $ingestTxCommitRaw -Field "ingest_tx_commit_ms")

Write-Host ("Ingestion budget observed full_index_ms: {0}" -f $fullIndexMs)
Write-Host ("Ingestion budget observed incremental_index_ms: {0}" -f $incrementalIndexMs)
Write-Host ("Ingestion budget observed peak_observed_rss_bytes: {0}" -f $peakRssBytes)

if ($fullIndexMs -gt $MaxFullIndexMs) {
    throw ("Ingestion budget failed: full_index_ms {0} > max {1}" -f $fullIndexMs, $MaxFullIndexMs)
}
if ($incrementalIndexMs -gt $MaxIncrementalIndexMs) {
    throw ("Ingestion budget failed: incremental_index_ms {0} > max {1}" -f $incrementalIndexMs, $MaxIncrementalIndexMs)
}
if ($peakRssBytes -gt $MaxPeakRssBytes) {
    throw ("Ingestion budget failed: peak_observed_rss_bytes {0} > max {1}" -f $peakRssBytes, $MaxPeakRssBytes)
}

Write-Host "Ingestion budget passed."
