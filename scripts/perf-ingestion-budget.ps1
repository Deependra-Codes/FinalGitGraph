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
    [double]$MinFullCommitThroughputPerSec = -1,
    [double]$MaxFileChangeExtractionDrainMs = -1,
    [int]$MaxOldestExtractionQueueAgeSeconds = 60,
    [switch]$FastMode,
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
    if ($FastMode) {
        $OutputPath = $defaultOutputPath.Replace(".txt", "-fast.txt")
    } else {
        $OutputPath = $defaultOutputPath
    }
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
if ($FastMode -and -not $PSBoundParameters.ContainsKey("MaxPeakRssBytes")) {
    $MaxPeakRssBytes = [UInt64]160000000
}
if (-not $PSBoundParameters.ContainsKey("MaxDiffShareRatio")) {
    if ($Lane -eq "warm") {
        $MaxDiffShareRatio = [Math]::Max($defaultMaxDiffShareRatio, 0.70)
    } else {
        $MaxDiffShareRatio = $defaultMaxDiffShareRatio
    }
}
if ($FastMode -and -not $PSBoundParameters.ContainsKey("MinFullCommitThroughputPerSec")) {
    $MinFullCommitThroughputPerSec = 12000
}
if ($FastMode -and -not $PSBoundParameters.ContainsKey("MaxFileChangeExtractionDrainMs")) {
    $MaxFileChangeExtractionDrainMs = 10000
}
if (-not $FastMode -and $MinFullCommitThroughputPerSec -lt 0) {
    $MinFullCommitThroughputPerSec = 0
}
if (-not $FastMode -and $MaxFileChangeExtractionDrainMs -lt 0) {
    $MaxFileChangeExtractionDrainMs = 0
}

Write-Host ("Ingestion budget profile: {0}" -f $Profile)
Write-Host ("Ingestion budget lane: {0}" -f $Lane)
Write-Host ("Ingestion budget fast mode: {0}" -f ($FastMode.IsPresent))
Write-Host (
    "Workload: cycles={0} feature_commits={1} incremental_commits={2} query_runs={3} window_limit={4} max_nodes={5}" `
        -f $cycles, $featureCommits, $incrementalCommits, $queryRuns, $windowLimit, $maxNodes
)
Write-Host (
    "Thresholds: max_full_index_ms={0} max_incremental_index_ms={1} max_peak_rss_bytes={2} max_diff_share_ratio={3} min_full_commit_throughput_per_sec={4} max_file_change_extraction_drain_ms={5} max_oldest_extraction_queue_age_seconds={6}" `
        -f $MaxFullIndexMs, $MaxIncrementalIndexMs, $MaxPeakRssBytes, $MaxDiffShareRatio, $MinFullCommitThroughputPerSec, $MaxFileChangeExtractionDrainMs, $MaxOldestExtractionQueueAgeSeconds
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

$previousFastMode = $env:GITGRAPH_INDEX_FAST_MODE
$previousWriteProfile = $env:GITGRAPH_SQLITE_WRITE_PROFILE
$previousWalkSort = $env:GITGRAPH_COMMIT_WALK_SORT
$previousPerfReportDockerOnly = $env:GITGRAPH_PERF_REPORT_DOCKER_ONLY
$previousDrainChunkSize = $env:GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE
$previousDrainChannelCapacity = $env:GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY
if ($FastMode) {
    $env:GITGRAPH_INDEX_FAST_MODE = "1"
    $env:GITGRAPH_SQLITE_WRITE_PROFILE = "throughput"
    $env:GITGRAPH_PERF_REPORT_DOCKER_ONLY = "1"
    if ([string]::IsNullOrWhiteSpace($previousWalkSort)) {
        $env:GITGRAPH_COMMIT_WALK_SORT = "breadth_first"
    }
    if ([string]::IsNullOrWhiteSpace($previousDrainChunkSize)) {
        $env:GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE = "64"
    }
    if ([string]::IsNullOrWhiteSpace($previousDrainChannelCapacity)) {
        $env:GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY = "32"
    }
}
try {
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
}
finally {
    if ($null -eq $previousFastMode) {
        Remove-Item Env:GITGRAPH_INDEX_FAST_MODE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_INDEX_FAST_MODE = $previousFastMode
    }
    if ($null -eq $previousWriteProfile) {
        Remove-Item Env:GITGRAPH_SQLITE_WRITE_PROFILE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_SQLITE_WRITE_PROFILE = $previousWriteProfile
    }
    if ($null -eq $previousWalkSort) {
        Remove-Item Env:GITGRAPH_COMMIT_WALK_SORT -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_COMMIT_WALK_SORT = $previousWalkSort
    }
    if ($null -eq $previousPerfReportDockerOnly) {
        Remove-Item Env:GITGRAPH_PERF_REPORT_DOCKER_ONLY -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_PERF_REPORT_DOCKER_ONLY = $previousPerfReportDockerOnly
    }
    if ($null -eq $previousDrainChunkSize) {
        Remove-Item Env:GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE = $previousDrainChunkSize
    }
    if ($null -eq $previousDrainChannelCapacity) {
        Remove-Item Env:GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY = $previousDrainChannelCapacity
    }
}

$fullIndexRaw = Get-ReportValue -Path $OutputPath -Key "full_index"
$incrementalIndexRaw = Get-ReportValue -Path $OutputPath -Key "incremental_index"
$peakRssRaw = Get-ReportValue -Path $OutputPath -Key "peak_observed_rss"
$fullCommitThroughputRaw = Get-ReportValue -Path $OutputPath -Key "commit_throughput_per_sec"
$fileChangeExtractionDrainRaw = Get-ReportValue -Path $OutputPath -Key "file_change_extraction_drain_ms"
$oldestExtractionQueueAgeRaw = Get-ReportValue -Path $OutputPath -Key "oldest_file_change_extraction_queue_age_seconds"
$coreEndToEndRaw = Get-ReportValue -Path $OutputPath -Key "core_end_to_end"
$fixtureGenerationRaw = Get-ReportValue -Path $OutputPath -Key "fixture_generation"
$ingestCommitWalkRaw = Get-ReportValue -Path $OutputPath -Key "ingest_commit_walk_ms"
$ingestDiffRaw = Get-ReportValue -Path $OutputPath -Key "ingest_file_diff_extract_ms"
$ingestUpsertRaw = Get-ReportValue -Path $OutputPath -Key "ingest_file_change_upsert_ms"
$ingestTxCommitRaw = Get-ReportValue -Path $OutputPath -Key "ingest_tx_commit_ms"

$fullIndexMs = Parse-DoubleOrThrow -Raw $fullIndexRaw -Field "full_index"
$incrementalIndexMs = Parse-DoubleOrThrow -Raw $incrementalIndexRaw -Field "incremental_index"
$peakRssBytes = [UInt64](Parse-DoubleOrThrow -Raw $peakRssRaw -Field "peak_observed_rss")
$fullCommitThroughputPerSec = Parse-DoubleOrThrow -Raw $fullCommitThroughputRaw -Field "commit_throughput_per_sec"
$fileChangeExtractionDrainMs = Parse-DoubleOrThrow -Raw $fileChangeExtractionDrainRaw -Field "file_change_extraction_drain_ms"
$oldestExtractionQueueAgeSeconds = [int](Parse-DoubleOrThrow -Raw $oldestExtractionQueueAgeRaw -Field "oldest_file_change_extraction_queue_age_seconds")
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
Write-Host ("Ingestion budget observed full_commit_throughput_per_sec: {0}" -f $fullCommitThroughputPerSec)
Write-Host ("Ingestion budget observed file_change_extraction_drain_ms: {0}" -f $fileChangeExtractionDrainMs)
Write-Host ("Ingestion budget observed oldest_file_change_extraction_queue_age_seconds: {0}" -f $oldestExtractionQueueAgeSeconds)
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
if ($FastMode -and $fullCommitThroughputPerSec -lt $MinFullCommitThroughputPerSec) {
    throw ("Ingestion budget fast-mode failed: full_commit_throughput_per_sec {0} < min {1}" -f $fullCommitThroughputPerSec, $MinFullCommitThroughputPerSec)
}
if ($FastMode -and $fileChangeExtractionDrainMs -gt $MaxFileChangeExtractionDrainMs) {
    throw ("Ingestion budget fast-mode failed: file_change_extraction_drain_ms {0} > max {1}" -f $fileChangeExtractionDrainMs, $MaxFileChangeExtractionDrainMs)
}
if ($FastMode -and $oldestExtractionQueueAgeSeconds -gt $MaxOldestExtractionQueueAgeSeconds) {
    throw ("Ingestion budget fast-mode failed: oldest_file_change_extraction_queue_age_seconds {0} > max {1}" -f $oldestExtractionQueueAgeSeconds, $MaxOldestExtractionQueueAgeSeconds)
}

Write-Host "Ingestion budget passed."
