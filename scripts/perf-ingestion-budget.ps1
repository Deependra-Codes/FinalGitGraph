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
    [int]$FileChangeCommitBatchSize = -1,
    [int]$RenameLimitLarge = -1,
    [int]$RenameLimitXLarge = -1,
    [switch]$EnableLsmRocksdb,
    [string]$LibclangPath = "",
    [double]$MinLsmShadowWriteAttempts = -1,
    [double]$MinLsmShadowWriteSuccessRatio = -1,
    [double]$MaxLsmShadowWriteErrors = -1,
    [double]$MaxLsmShadowDisabledBatches = -1,
    [double]$MaxLsmMaterializationErrors = -1,
    [switch]$FastMode,
    [switch]$PreconditionCommitGraph,
    [switch]$ProbeOnly
)

$ErrorActionPreference = "Stop"

if ($env:OS -eq "Windows_NT") {
    if (-not (Test-Path "D:\")) {
        throw "D: drive is required on Windows for this script."
    }
    $preferredTempRoot = "D:\temp\gitgraph-lab"
    if (-not (Test-Path $preferredTempRoot)) {
        New-Item -Path $preferredTempRoot -ItemType Directory -Force | Out-Null
    }
    $env:TEMP = $preferredTempRoot
    $env:TMP = $preferredTempRoot
}

if ([string]::IsNullOrWhiteSpace($CargoTargetRoot)) {
    if ($env:OS -eq "Windows_NT") {
        $CargoTargetRoot = "D:\cargo-target\gitgraph-lab"
    }
}

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
if ($FastMode -and -not $EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MinFullCommitThroughputPerSec")) {
    $MinFullCommitThroughputPerSec = 12000
}
if ($EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MinFullCommitThroughputPerSec")) {
    $MinFullCommitThroughputPerSec = 500
}
if ($FastMode -and -not $PSBoundParameters.ContainsKey("MaxFileChangeExtractionDrainMs")) {
    $MaxFileChangeExtractionDrainMs = 10000
}
if ($EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MinLsmShadowWriteAttempts")) {
    $MinLsmShadowWriteAttempts = 1
}
if ($EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MinLsmShadowWriteSuccessRatio")) {
    $MinLsmShadowWriteSuccessRatio = 0.95
}
if ($EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MaxLsmShadowWriteErrors")) {
    $MaxLsmShadowWriteErrors = 0
}
if ($EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MaxLsmShadowDisabledBatches")) {
    $MaxLsmShadowDisabledBatches = 0
}
if ($EnableLsmRocksdb -and -not $PSBoundParameters.ContainsKey("MaxLsmMaterializationErrors")) {
    $MaxLsmMaterializationErrors = 0
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
Write-Host (
    "CPS tuning overrides: file_change_commit_batch_size={0} rename_limit_large={1} rename_limit_xlarge={2}" `
        -f $FileChangeCommitBatchSize, $RenameLimitLarge, $RenameLimitXLarge
)
Write-Host (
    "LSM options: enable_lsm_rocksdb={0} libclang_path={1} min_lsm_shadow_write_attempts={2} min_lsm_shadow_write_success_ratio={3} max_lsm_shadow_write_errors={4} max_lsm_shadow_disabled_batches={5} max_lsm_materialization_errors={6}" `
        -f $EnableLsmRocksdb.IsPresent, $LibclangPath, $MinLsmShadowWriteAttempts, $MinLsmShadowWriteSuccessRatio, $MaxLsmShadowWriteErrors, $MaxLsmShadowDisabledBatches, $MaxLsmMaterializationErrors
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
$previousFileChangeCommitBatchSize = $env:GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE
$previousRenameLimitLarge = $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_LARGE
$previousRenameLimitXLarge = $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_XLARGE
$previousLsmShadowEnable = $env:GITGRAPH_LSM_SHADOW_ENABLE
$previousIngestPrimary = $env:GITGRAPH_INGEST_PRIMARY
if ($FastMode) {
    $env:GITGRAPH_INDEX_FAST_MODE = "1"
    $env:GITGRAPH_SQLITE_WRITE_PROFILE = "throughput"
    if (-not $EnableLsmRocksdb) {
        $env:GITGRAPH_PERF_REPORT_DOCKER_ONLY = "1"
    }
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
if ($FileChangeCommitBatchSize -gt 0) {
    $env:GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = $FileChangeCommitBatchSize.ToString()
}
if ($RenameLimitLarge -gt 0) {
    $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_LARGE = $RenameLimitLarge.ToString()
}
if ($RenameLimitXLarge -gt 0) {
    $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_XLARGE = $RenameLimitXLarge.ToString()
}
if ($EnableLsmRocksdb) {
    $env:GITGRAPH_LSM_SHADOW_ENABLE = "1"
    $env:GITGRAPH_INGEST_PRIMARY = "rocksdb"
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
        -WarmFixture:($Lane -eq "warm") `
        -EnableLsmRocksdb:$EnableLsmRocksdb `
        -LibclangPath $LibclangPath
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
    if ($null -eq $previousFileChangeCommitBatchSize) {
        Remove-Item Env:GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = $previousFileChangeCommitBatchSize
    }
    if ($null -eq $previousRenameLimitLarge) {
        Remove-Item Env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_LARGE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_LARGE = $previousRenameLimitLarge
    }
    if ($null -eq $previousRenameLimitXLarge) {
        Remove-Item Env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_XLARGE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_XLARGE = $previousRenameLimitXLarge
    }
    if ($null -eq $previousLsmShadowEnable) {
        Remove-Item Env:GITGRAPH_LSM_SHADOW_ENABLE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_LSM_SHADOW_ENABLE = $previousLsmShadowEnable
    }
    if ($null -eq $previousIngestPrimary) {
        Remove-Item Env:GITGRAPH_INGEST_PRIMARY -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_INGEST_PRIMARY = $previousIngestPrimary
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
$lsmShadowWriteAttemptsRaw = Get-ReportValue -Path $OutputPath -Key "lsm_shadow_write_attempts"
$lsmShadowWriteSuccessRaw = Get-ReportValue -Path $OutputPath -Key "lsm_shadow_write_success"
$lsmShadowWriteErrorsRaw = Get-ReportValue -Path $OutputPath -Key "lsm_shadow_write_errors"
$lsmShadowDisabledBatchesRaw = Get-ReportValue -Path $OutputPath -Key "lsm_shadow_disabled_batches"
$lsmMaterializationErrorsRaw = Get-ReportValue -Path $OutputPath -Key "lsm_materialization_errors"

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
$lsmShadowWriteAttempts = Parse-DoubleOrThrow -Raw $lsmShadowWriteAttemptsRaw -Field "lsm_shadow_write_attempts"
$lsmShadowWriteSuccess = Parse-DoubleOrThrow -Raw $lsmShadowWriteSuccessRaw -Field "lsm_shadow_write_success"
$lsmShadowWriteErrors = Parse-DoubleOrThrow -Raw $lsmShadowWriteErrorsRaw -Field "lsm_shadow_write_errors"
$lsmShadowDisabledBatches = Parse-DoubleOrThrow -Raw $lsmShadowDisabledBatchesRaw -Field "lsm_shadow_disabled_batches"
$lsmMaterializationErrors = Parse-DoubleOrThrow -Raw $lsmMaterializationErrorsRaw -Field "lsm_materialization_errors"
$lsmShadowWriteSuccessRatio = if ($lsmShadowWriteAttempts -le 0) { 0.0 } else { $lsmShadowWriteSuccess / $lsmShadowWriteAttempts }
$diffShareRatio = if ($coreEndToEndMs -le 0) { 0.0 } else { $ingestDiffMs / $coreEndToEndMs }

Write-Host ("Ingestion budget observed full_index_ms: {0}" -f $fullIndexMs)
Write-Host ("Ingestion budget observed incremental_index_ms: {0}" -f $incrementalIndexMs)
Write-Host ("Ingestion budget observed peak_observed_rss_bytes: {0}" -f $peakRssBytes)
Write-Host ("Ingestion budget observed full_commit_throughput_per_sec: {0}" -f $fullCommitThroughputPerSec)
Write-Host ("Ingestion budget observed file_change_extraction_drain_ms: {0}" -f $fileChangeExtractionDrainMs)
Write-Host ("Ingestion budget observed oldest_file_change_extraction_queue_age_seconds: {0}" -f $oldestExtractionQueueAgeSeconds)
Write-Host ("Ingestion budget observed fixture_generation_ms: {0}" -f $fixtureGenerationMs)
Write-Host ("Ingestion budget observed diff_share_ratio: {0}" -f ([Math]::Round($diffShareRatio, 4)))
Write-Host ("Ingestion budget observed lsm_shadow_write_attempts: {0}" -f $lsmShadowWriteAttempts)
Write-Host ("Ingestion budget observed lsm_shadow_write_success: {0}" -f $lsmShadowWriteSuccess)
Write-Host ("Ingestion budget observed lsm_shadow_write_errors: {0}" -f $lsmShadowWriteErrors)
Write-Host ("Ingestion budget observed lsm_shadow_disabled_batches: {0}" -f $lsmShadowDisabledBatches)
Write-Host ("Ingestion budget observed lsm_materialization_errors: {0}" -f $lsmMaterializationErrors)
Write-Host ("Ingestion budget observed lsm_shadow_write_success_ratio: {0}" -f ([Math]::Round($lsmShadowWriteSuccessRatio, 4)))

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
if ($MinLsmShadowWriteAttempts -ge 0 -and $lsmShadowWriteAttempts -lt $MinLsmShadowWriteAttempts) {
    throw ("Ingestion budget failed: lsm_shadow_write_attempts {0} < min {1}" -f $lsmShadowWriteAttempts, $MinLsmShadowWriteAttempts)
}
if ($MinLsmShadowWriteSuccessRatio -ge 0 -and $lsmShadowWriteSuccessRatio -lt $MinLsmShadowWriteSuccessRatio) {
    throw ("Ingestion budget failed: lsm_shadow_write_success_ratio {0} < min {1}" -f $lsmShadowWriteSuccessRatio, $MinLsmShadowWriteSuccessRatio)
}
if ($MaxLsmShadowWriteErrors -ge 0 -and $lsmShadowWriteErrors -gt $MaxLsmShadowWriteErrors) {
    throw ("Ingestion budget failed: lsm_shadow_write_errors {0} > max {1}" -f $lsmShadowWriteErrors, $MaxLsmShadowWriteErrors)
}
if ($MaxLsmShadowDisabledBatches -ge 0 -and $lsmShadowDisabledBatches -gt $MaxLsmShadowDisabledBatches) {
    throw ("Ingestion budget failed: lsm_shadow_disabled_batches {0} > max {1}" -f $lsmShadowDisabledBatches, $MaxLsmShadowDisabledBatches)
}
if ($MaxLsmMaterializationErrors -ge 0 -and $lsmMaterializationErrors -gt $MaxLsmMaterializationErrors) {
    throw ("Ingestion budget failed: lsm_materialization_errors {0} > max {1}" -f $lsmMaterializationErrors, $MaxLsmMaterializationErrors)
}

Write-Host "Ingestion budget passed."
