# platform: any
param(
    [string]$CargoTargetRoot = "",
    [ValidateSet("25k", "90k", "100k")]
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
    [switch]$FastMode,
    [switch]$PreconditionCommitGraph,
    [ValidateSet("system", "mimalloc")]
    [string]$Allocator = "system",
    [switch]$NativeCpu,
    [switch]$HardwareMax,
    [switch]$ProbeOnly
)

$ErrorActionPreference = "Stop"

if ($HardwareMax) {
    $FastMode = [System.Management.Automation.SwitchParameter]::new($true)
    $PreconditionCommitGraph = [System.Management.Automation.SwitchParameter]::new($true)
    $Allocator = "mimalloc"
    $NativeCpu = [System.Management.Automation.SwitchParameter]::new($true)
}

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

if ($Profile -eq "100k") {
    $defaultOutputPath = "tmp/perf-report-ingestion-budget-100k.txt"
    $cycles = 12500
    $featureCommits = 6
    $incrementalCommits = 10000
    $queryRuns = 20
    $windowLimit = 1200
    $maxNodes = 100000
    $defaultMaxFullIndexMs = 120000
    $defaultMaxIncrementalIndexMs = 90000
    $defaultMaxPeakRssBytes = [UInt64]300000000
    $defaultMaxDiffShareRatio = 0.55
} elseif ($Profile -eq "90k") {
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
    if ($HardwareMax) {
        $OutputPath = $defaultOutputPath.Replace(".txt", "-hardware.txt")
    } elseif ($FastMode) {
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
if ($FastMode -and -not $HardwareMax -and -not $PSBoundParameters.ContainsKey("MaxPeakRssBytes")) {
    $MaxPeakRssBytes = [UInt64]160000000
}
if (-not $PSBoundParameters.ContainsKey("MaxDiffShareRatio")) {
    if ($Lane -eq "warm") {
        $MaxDiffShareRatio = [Math]::Max($defaultMaxDiffShareRatio, 0.70)
    } else {
        $MaxDiffShareRatio = $defaultMaxDiffShareRatio
    }
}
if ($HardwareMax -and -not $PSBoundParameters.ContainsKey("MinFullCommitThroughputPerSec")) {
    $MinFullCommitThroughputPerSec = 100000
}
if ($FastMode -and -not $HardwareMax -and -not $PSBoundParameters.ContainsKey("MinFullCommitThroughputPerSec")) {
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
Write-Host ("Ingestion hardware max: {0}" -f ([bool]$HardwareMax))
Write-Host ("Ingestion allocator: {0}" -f $Allocator)
Write-Host ("Ingestion native CPU: {0}" -f ([bool]$NativeCpu))
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
            -Allocator $Allocator `
            -NativeCpu:$NativeCpu `
            -HardwareMax:$HardwareMax `
            -OutputPath ($OutputPath + ".bootstrap.txt")
        if ($LASTEXITCODE -ne 0) {
            throw "Warm lane bootstrap perf-report execution failed."
        }
    }
}

$previousWriteProfile = $env:GITGRAPH_INGEST_WRITE_PROFILE
$previousWalkSort = $env:GITGRAPH_COMMIT_WALK_SORT
$previousPerfReportDockerOnly = $env:GITGRAPH_PERF_REPORT_DOCKER_ONLY
$previousDrainChunkSize = $env:GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE
$previousDrainChannelCapacity = $env:GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY
$previousFileChangeCommitBatchSize = $env:GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE
$previousRenameLimitLarge = $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_LARGE
$previousRenameLimitXLarge = $env:GITGRAPH_FILE_CHANGE_RENAME_LIMIT_XLARGE
if ($FastMode) {
    $env:GITGRAPH_INGEST_WRITE_PROFILE = "throughput"
    if (-not $HardwareMax) {
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
        -Allocator $Allocator `
        -NativeCpu:$NativeCpu `
        -HardwareMax:$HardwareMax
    if ($LASTEXITCODE -ne 0) {
        throw "perf-report execution failed for ingestion budget lane."
    }
}
finally {
    if ($null -eq $previousWriteProfile) {
        Remove-Item Env:GITGRAPH_INGEST_WRITE_PROFILE -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_INGEST_WRITE_PROFILE = $previousWriteProfile
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
$fullCommitBottleneckPhaseRaw = Get-ReportValue -Path $OutputPath -Key "full_commit_bottleneck_phase"
$fullCommitBottleneckPhaseMsRaw = Get-ReportValue -Path $OutputPath -Key "full_commit_bottleneck_phase_ms"
$fullCommitBottleneckPhaseShareRaw = Get-ReportValue -Path $OutputPath -Key "full_commit_bottleneck_phase_share"
$incrementalCommitBottleneckPhaseRaw = Get-ReportValue -Path $OutputPath -Key "incremental_commit_bottleneck_phase"
$incrementalCommitBottleneckPhaseMsRaw = Get-ReportValue -Path $OutputPath -Key "incremental_commit_bottleneck_phase_ms"
$incrementalCommitBottleneckPhaseShareRaw = Get-ReportValue -Path $OutputPath -Key "incremental_commit_bottleneck_phase_share"
$fullCommitWalkThroughputRaw = Get-ReportValue -Path $OutputPath -Key "full_commit_walk_commits_per_sec"
$fullCommitUpsertThroughputRaw = Get-ReportValue -Path $OutputPath -Key "full_commit_upsert_commits_per_sec"
$fullDiffExtractThroughputRaw = Get-ReportValue -Path $OutputPath -Key "full_file_diff_extract_commits_per_sec"
$fullFileChangeUpsertThroughputRaw = Get-ReportValue -Path $OutputPath -Key "full_file_change_upsert_commits_per_sec"
$fullTxCommitThroughputRaw = Get-ReportValue -Path $OutputPath -Key "full_tx_commit_commits_per_sec"
$incrementalCommitWalkThroughputRaw = Get-ReportValue -Path $OutputPath -Key "incremental_commit_walk_commits_per_sec"
$incrementalCommitUpsertThroughputRaw = Get-ReportValue -Path $OutputPath -Key "incremental_commit_upsert_commits_per_sec"
$incrementalDiffExtractThroughputRaw = Get-ReportValue -Path $OutputPath -Key "incremental_file_diff_extract_commits_per_sec"
$incrementalFileChangeUpsertThroughputRaw = Get-ReportValue -Path $OutputPath -Key "incremental_file_change_upsert_commits_per_sec"
$incrementalTxCommitThroughputRaw = Get-ReportValue -Path $OutputPath -Key "incremental_tx_commit_commits_per_sec"

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
$fullCommitBottleneckPhaseMs = Parse-DoubleOrThrow -Raw $fullCommitBottleneckPhaseMsRaw -Field "full_commit_bottleneck_phase_ms"
$fullCommitBottleneckPhaseShare = Parse-DoubleOrThrow -Raw $fullCommitBottleneckPhaseShareRaw -Field "full_commit_bottleneck_phase_share"
$incrementalCommitBottleneckPhaseMs = Parse-DoubleOrThrow -Raw $incrementalCommitBottleneckPhaseMsRaw -Field "incremental_commit_bottleneck_phase_ms"
$incrementalCommitBottleneckPhaseShare = Parse-DoubleOrThrow -Raw $incrementalCommitBottleneckPhaseShareRaw -Field "incremental_commit_bottleneck_phase_share"
$fullCommitWalkThroughput = Parse-DoubleOrThrow -Raw $fullCommitWalkThroughputRaw -Field "full_commit_walk_commits_per_sec"
$fullCommitUpsertThroughput = Parse-DoubleOrThrow -Raw $fullCommitUpsertThroughputRaw -Field "full_commit_upsert_commits_per_sec"
$fullDiffExtractThroughput = Parse-DoubleOrThrow -Raw $fullDiffExtractThroughputRaw -Field "full_file_diff_extract_commits_per_sec"
$fullFileChangeUpsertThroughput = Parse-DoubleOrThrow -Raw $fullFileChangeUpsertThroughputRaw -Field "full_file_change_upsert_commits_per_sec"
$fullTxCommitThroughput = Parse-DoubleOrThrow -Raw $fullTxCommitThroughputRaw -Field "full_tx_commit_commits_per_sec"
$incrementalCommitWalkThroughput = Parse-DoubleOrThrow -Raw $incrementalCommitWalkThroughputRaw -Field "incremental_commit_walk_commits_per_sec"
$incrementalCommitUpsertThroughput = Parse-DoubleOrThrow -Raw $incrementalCommitUpsertThroughputRaw -Field "incremental_commit_upsert_commits_per_sec"
$incrementalDiffExtractThroughput = Parse-DoubleOrThrow -Raw $incrementalDiffExtractThroughputRaw -Field "incremental_file_diff_extract_commits_per_sec"
$incrementalFileChangeUpsertThroughput = Parse-DoubleOrThrow -Raw $incrementalFileChangeUpsertThroughputRaw -Field "incremental_file_change_upsert_commits_per_sec"
$incrementalTxCommitThroughput = Parse-DoubleOrThrow -Raw $incrementalTxCommitThroughputRaw -Field "incremental_tx_commit_commits_per_sec"
$diffShareRatio = if ($coreEndToEndMs -le 0) { 0.0 } else { $ingestDiffMs / $coreEndToEndMs }

Write-Host ("Ingestion budget observed full_index_ms: {0}" -f $fullIndexMs)
Write-Host ("Ingestion budget observed incremental_index_ms: {0}" -f $incrementalIndexMs)
Write-Host ("Ingestion budget observed peak_observed_rss_bytes: {0}" -f $peakRssBytes)
Write-Host ("Ingestion budget observed full_commit_throughput_per_sec: {0}" -f $fullCommitThroughputPerSec)
Write-Host ("Ingestion budget observed file_change_extraction_drain_ms: {0}" -f $fileChangeExtractionDrainMs)
Write-Host ("Ingestion budget observed oldest_file_change_extraction_queue_age_seconds: {0}" -f $oldestExtractionQueueAgeSeconds)
Write-Host ("Ingestion budget observed fixture_generation_ms: {0}" -f $fixtureGenerationMs)
Write-Host ("Ingestion budget observed diff_share_ratio: {0}" -f ([Math]::Round($diffShareRatio, 4)))
Write-Host ("Ingestion throughput phases (full) cps: walk={0} upsert={1} diff_extract={2} file_change_upsert={3} tx_commit={4}" -f $fullCommitWalkThroughput, $fullCommitUpsertThroughput, $fullDiffExtractThroughput, $fullFileChangeUpsertThroughput, $fullTxCommitThroughput)
Write-Host ("Ingestion throughput phases (incremental) cps: walk={0} upsert={1} diff_extract={2} file_change_upsert={3} tx_commit={4}" -f $incrementalCommitWalkThroughput, $incrementalCommitUpsertThroughput, $incrementalDiffExtractThroughput, $incrementalFileChangeUpsertThroughput, $incrementalTxCommitThroughput)
Write-Host ("Ingestion bottleneck (full): phase={0} phase_ms={1} phase_share={2}" -f $fullCommitBottleneckPhaseRaw, $fullCommitBottleneckPhaseMs, $fullCommitBottleneckPhaseShare)
Write-Host ("Ingestion bottleneck (incremental): phase={0} phase_ms={1} phase_share={2}" -f $incrementalCommitBottleneckPhaseRaw, $incrementalCommitBottleneckPhaseMs, $incrementalCommitBottleneckPhaseShare)

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

