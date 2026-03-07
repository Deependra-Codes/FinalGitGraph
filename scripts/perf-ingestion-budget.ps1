# platform: any
param(
    [string]$CargoTargetRoot = "",
    [ValidateSet("25k", "90k")]
    [string]$Profile = "25k",
    [ValidateSet("cold", "warm")]
    [string]$Lane = "cold",
    [string]$OutputPath = "",
    [double]$MaxFullIndexMs = 45000,
    [double]$MaxIncrementalIndexMs = 15000,
    [UInt64]$MaxPeakRssBytes = 60000000,
    [double]$MaxDiffShareRatio = -1,
    [double]$MaxWarmFixtureGenerationMs = 1000,
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
    $defaultMaxDiffShareRatio = 0.55
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
    $defaultMaxDiffShareRatio = 0.55
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
if (-not $PSBoundParameters.ContainsKey("MaxDiffShareRatio")) {
    if ($Lane -eq "warm") {
        $MaxDiffShareRatio = [Math]::Max($defaultMaxDiffShareRatio, 0.70)
    } else {
        $MaxDiffShareRatio = $defaultMaxDiffShareRatio
    }
}

Write-Host ("Ingestion budget profile: {0}" -f $Profile)
Write-Host ("Ingestion budget lane: {0}" -f $Lane)
Write-Host (
    "Workload: cycles={0} feature_commits={1} incremental_commits={2} query_runs={3} window_limit={4} max_nodes={5}" `
        -f $cycles, $featureCommits, $incrementalCommits, $queryRuns, $windowLimit, $maxNodes
)
Write-Host (
    "Thresholds: max_full_index_ms={0} max_incremental_index_ms={1} max_peak_rss_bytes={2} max_diff_share_ratio={3}" `
        -f $MaxFullIndexMs, $MaxIncrementalIndexMs, $MaxPeakRssBytes, $MaxDiffShareRatio
)

$fixtureRoot = ""
if ($Lane -eq "warm") {
    $fixtureRoot = Join-Path "tmp/perf-fixtures" ("ingestion-budget-{0}" -f $Profile)
    $warmRepoGitDir = Join-Path $fixtureRoot "repo/.git"
    if (-not (Test-Path $warmRepoGitDir)) {
        Write-Host "Warm lane bootstrap: generating reusable fixture..."
        & "$PSScriptRoot\perf-report.ps1" `
            -CargoTargetRoot $CargoTargetRoot `
            -Cycles $cycles `
            -FeatureCommits $featureCommits `
            -IncrementalCommits $incrementalCommits `
            -QueryRuns $queryRuns `
            -WindowLimit $windowLimit `
            -MaxNodes $maxNodes `
            -PreconditionCommitGraph:$PreconditionCommitGraph `
            -FixtureRoot $fixtureRoot `
            -KeepFixture `
            -OutputPath ($OutputPath + ".bootstrap.txt")
        if ($LASTEXITCODE -ne 0) {
            throw "Warm lane bootstrap perf-report execution failed."
        }
    }
}

& "$PSScriptRoot\perf-report.ps1" `
    -CargoTargetRoot $CargoTargetRoot `
    -Cycles $cycles `
    -FeatureCommits $featureCommits `
    -IncrementalCommits $incrementalCommits `
    -QueryRuns $queryRuns `
    -WindowLimit $windowLimit `
    -MaxNodes $maxNodes `
    -PreconditionCommitGraph:$PreconditionCommitGraph `
    -OutputPath $OutputPath `
    -FixtureRoot $fixtureRoot `
    -WarmFixture:($Lane -eq "warm")
if ($LASTEXITCODE -ne 0) {
    throw "perf-report execution failed for ingestion budget lane."
}

$fullIndexRaw = Get-ReportValue -Path $OutputPath -Key "full_index"
$incrementalIndexRaw = Get-ReportValue -Path $OutputPath -Key "incremental_index"
$peakRssRaw = Get-ReportValue -Path $OutputPath -Key "peak_observed_rss"
$coreEndToEndRaw = Get-ReportValue -Path $OutputPath -Key "core_end_to_end"
$fixtureGenerationRaw = Get-ReportValue -Path $OutputPath -Key "fixture_generation"
$ingestCommitWalkRaw = Get-ReportValue -Path $OutputPath -Key "ingest_commit_walk_ms"
$ingestDiffRaw = Get-ReportValue -Path $OutputPath -Key "ingest_file_diff_extract_ms"
$ingestUpsertRaw = Get-ReportValue -Path $OutputPath -Key "ingest_file_change_upsert_ms"
$ingestTxCommitRaw = Get-ReportValue -Path $OutputPath -Key "ingest_tx_commit_ms"

$fullIndexMs = Parse-DoubleOrThrow -Raw $fullIndexRaw -Field "full_index"
$incrementalIndexMs = Parse-DoubleOrThrow -Raw $incrementalIndexRaw -Field "incremental_index"
$peakRssBytes = [UInt64](Parse-DoubleOrThrow -Raw $peakRssRaw -Field "peak_observed_rss")
$coreEndToEndMs = Parse-DoubleOrThrow -Raw $coreEndToEndRaw -Field "core_end_to_end"
$fixtureGenerationMs = Parse-DoubleOrThrow -Raw $fixtureGenerationRaw -Field "fixture_generation"
[void](Parse-DoubleOrThrow -Raw $ingestCommitWalkRaw -Field "ingest_commit_walk_ms")
$ingestDiffMs = Parse-DoubleOrThrow -Raw $ingestDiffRaw -Field "ingest_file_diff_extract_ms"
[void](Parse-DoubleOrThrow -Raw $ingestUpsertRaw -Field "ingest_file_change_upsert_ms")
[void](Parse-DoubleOrThrow -Raw $ingestTxCommitRaw -Field "ingest_tx_commit_ms")
$diffShareRatio = if ($coreEndToEndMs -le 0) { 0.0 } else { $ingestDiffMs / $coreEndToEndMs }

Write-Host ("Ingestion budget observed full_index_ms: {0}" -f $fullIndexMs)
Write-Host ("Ingestion budget observed incremental_index_ms: {0}" -f $incrementalIndexMs)
Write-Host ("Ingestion budget observed peak_observed_rss_bytes: {0}" -f $peakRssBytes)
Write-Host ("Ingestion budget observed fixture_generation_ms: {0}" -f $fixtureGenerationMs)
Write-Host ("Ingestion budget observed diff_share_ratio: {0}" -f ([Math]::Round($diffShareRatio, 4)))

if ($fullIndexMs -gt $MaxFullIndexMs) {
    throw ("Ingestion budget failed: full_index_ms {0} > max {1}" -f $fullIndexMs, $MaxFullIndexMs)
}
if ($incrementalIndexMs -gt $MaxIncrementalIndexMs) {
    throw ("Ingestion budget failed: incremental_index_ms {0} > max {1}" -f $incrementalIndexMs, $MaxIncrementalIndexMs)
}
if ($peakRssBytes -gt $MaxPeakRssBytes) {
    throw ("Ingestion budget failed: peak_observed_rss_bytes {0} > max {1}" -f $peakRssBytes, $MaxPeakRssBytes)
}
if ($diffShareRatio -gt $MaxDiffShareRatio) {
    throw ("Ingestion budget failed: diff_share_ratio {0} > max {1}" -f $diffShareRatio, $MaxDiffShareRatio)
}
if ($Lane -eq "warm" -and $fixtureGenerationMs -gt $MaxWarmFixtureGenerationMs) {
    throw ("Ingestion budget warm-lane failed: fixture_generation_ms {0} > max {1}" -f $fixtureGenerationMs, $MaxWarmFixtureGenerationMs)
}

Write-Host "Ingestion budget passed."
