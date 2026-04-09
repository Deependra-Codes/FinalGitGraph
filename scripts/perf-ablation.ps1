# platform: any
param(
    [int]$Cycles = 2500,
    [int]$FeatureCommits = 6,
    [int]$IncrementalCommits = 2500,
    [int]$QueryRuns = 20,
    [int]$WindowLimit = 500,
    [int]$MaxNodes = 25000,
    [int]$Runs = 3,
    [string]$OutputPath = "tmp/perf-ablation-25k.md",
    [string]$OnlyScenario = "",
    [string]$RequiredScenario = "",
    [double]$MinAggregateCps = -1
)

$ErrorActionPreference = "Stop"

function Get-ReportValue {
    param(
        [string]$Path,
        [string]$Key
    )
    $line = Select-String -Path $Path -Pattern ("^\s*" + [Regex]::Escape($Key) + ":\s*(.+)$") | Select-Object -First 1
    if ($null -eq $line) {
        throw "Missing key in report: $Key ($Path)"
    }
    $line.Matches[0].Groups[1].Value.Trim()
}

function Get-ReportValueOrDefault {
    param(
        [string]$Path,
        [string]$Key,
        [string]$DefaultValue
    )
    $line = Select-String -Path $Path -Pattern ("^\s*" + [Regex]::Escape($Key) + ":\s*(.+)$") | Select-Object -First 1
    if ($null -eq $line) {
        return $DefaultValue
    }
    $line.Matches[0].Groups[1].Value.Trim()
}

function Parse-Double {
    param([string]$Raw)
    $value = 0.0
    if (-not [double]::TryParse($Raw, [System.Globalization.NumberStyles]::Float, [System.Globalization.CultureInfo]::InvariantCulture, [ref]$value)) {
        throw "Failed to parse numeric value: $Raw"
    }
    $value
}

function Median-Double {
    param([double[]]$Values)
    if ($null -eq $Values -or $Values.Count -eq 0) {
        return 0.0
    }
    $sorted = @($Values | Sort-Object)
    $count = $sorted.Length
    $middle = [int][Math]::Floor($count / 2.0)
    if ($count % 2 -eq 1) {
        return $sorted[$middle]
    }
    $left = $sorted[$middle - 1]
    $right = $sorted[$middle]
    return ($left + $right) / 2.0
}

function Format-DoubleInvariant {
    param([double]$Value)
    [string]::Format([System.Globalization.CultureInfo]::InvariantCulture, "{0:0.###}", $Value)
}

function Mode-String {
    param([string[]]$Values)
    if ($null -eq $Values -or $Values.Count -eq 0) {
        return "n/a"
    }
    $grouped = $Values | Group-Object | Sort-Object -Property Count, Name -Descending
    return $grouped[0].Name
}

function Invoke-Scenario {
    param(
        [string]$ScenarioName,
        [hashtable]$EnvVars
    )

    $scenarioDir = Join-Path "tmp/perf-ablation" $ScenarioName
    New-Item -Path $scenarioDir -ItemType Directory -Force | Out-Null

    $aggregateCps = New-Object System.Collections.Generic.List[double]
    $fullCps = New-Object System.Collections.Generic.List[double]
    $coreMs = New-Object System.Collections.Generic.List[double]
    $bottleneckPhase = New-Object System.Collections.Generic.List[string]
    $bottleneckMs = New-Object System.Collections.Generic.List[double]
    $walkScope = New-Object System.Collections.Generic.List[string]
    $walkTipCount = New-Object System.Collections.Generic.List[double]
    $prunedParentExpansions = New-Object System.Collections.Generic.List[double]

    $allKeys = @(
        "GITGRAPH_FILE_CHANGE_SYNC_STRICT",
        "GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH",
        "GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO",
        "GITGRAPH_INDEX_FAST_MODE",
        "GITGRAPH_LEVEL3_PACK_AWARE_INGEST",
        "GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER",
        "GITGRAPH_COMMIT_WALK_SORT",
        "GITGRAPH_COMMIT_WALK_SCOPE",
        "GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE",
        "GITGRAPH_FILE_CHANGE_PARALLEL_THREADS"
    )
    $previous = @{}
    foreach ($key in $allKeys) {
        $previous[$key] = [Environment]::GetEnvironmentVariable($key)
        Remove-Item ("Env:{0}" -f $key) -ErrorAction SilentlyContinue
    }
    foreach ($entry in $EnvVars.GetEnumerator()) {
        if ([string]::IsNullOrWhiteSpace($entry.Value)) {
            Remove-Item ("Env:{0}" -f $entry.Key) -ErrorAction SilentlyContinue
        } else {
            Set-Item -Path ("Env:{0}" -f $entry.Key) -Value $entry.Value
        }
    }

    try {
        for ($run = 1; $run -le $Runs; $run++) {
            Write-Host ("[{0}] {1} run {2}/{3}" -f (Get-Date -Format "HH:mm:ss"), $ScenarioName, $run, $Runs)
            $reportPath = Join-Path $scenarioDir ("run-{0}.txt" -f $run)
            & "$PSScriptRoot\perf-report.ps1" `
                -Cycles $Cycles `
                -FeatureCommits $FeatureCommits `
                -IncrementalCommits $IncrementalCommits `
                -QueryRuns $QueryRuns `
                -WindowLimit $WindowLimit `
                -MaxNodes $MaxNodes `
                -OutputPath $reportPath
            if ($LASTEXITCODE -ne 0) {
                throw "perf-report failed for scenario '$ScenarioName' run $run"
            }

            $aggregateCps.Add((Parse-Double (Get-ReportValue -Path $reportPath -Key "aggregate_commit_throughput_per_sec")))
            $fullCps.Add((Parse-Double (Get-ReportValue -Path $reportPath -Key "full_commit_throughput_per_sec")))
            $coreMs.Add((Parse-Double (Get-ReportValue -Path $reportPath -Key "core_end_to_end")))
            $bottleneckPhase.Add((Get-ReportValue -Path $reportPath -Key "aggregate_commit_bottleneck_phase"))
            $bottleneckMs.Add((Parse-Double (Get-ReportValue -Path $reportPath -Key "aggregate_commit_bottleneck_phase_ms")))
            $walkScope.Add((Get-ReportValueOrDefault -Path $reportPath -Key "full_commit_walk_scope" -DefaultValue "n/a"))
            $walkTipCount.Add((Parse-Double (Get-ReportValueOrDefault -Path $reportPath -Key "aggregate_commit_walk_tip_count" -DefaultValue "0")))
            $prunedParentExpansions.Add((Parse-Double (Get-ReportValueOrDefault -Path $reportPath -Key "aggregate_commit_walk_pruned_parent_expansions" -DefaultValue "0")))
        }
    }
    finally {
        foreach ($key in $allKeys) {
            if ($null -eq $previous[$key]) {
                Remove-Item ("Env:{0}" -f $key) -ErrorAction SilentlyContinue
            } else {
                Set-Item -Path ("Env:{0}" -f $key) -Value $previous[$key]
            }
        }
    }

    return @{
        Scenario = $ScenarioName
        AggregateCpsMedian = [Math]::Round((Median-Double $aggregateCps), 3)
        FullCpsMedian = [Math]::Round((Median-Double $fullCps), 3)
        CoreEndToEndMsMedian = [Math]::Round((Median-Double $coreMs), 3)
        BottleneckPhase = Mode-String $bottleneckPhase
        BottleneckMsMedian = [Math]::Round((Median-Double $bottleneckMs), 3)
        WalkScopeMode = Mode-String $walkScope
        AggregateWalkTipCountMedian = [Math]::Round((Median-Double $walkTipCount), 3)
        AggregatePrunedParentExpansionsMedian = [Math]::Round((Median-Double $prunedParentExpansions), 3)
        AggregateCpsRaw = (($aggregateCps | ForEach-Object { Format-DoubleInvariant $_ }) -join ", ")
    }
}

$scenarios = @(
    @{ Name = "baseline"; Env = @{
            GITGRAPH_FILE_CHANGE_SYNC_STRICT = "1"
            GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "1"
            GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "0"
            GITGRAPH_INDEX_FAST_MODE = "0"
            GITGRAPH_LEVEL3_PACK_AWARE_INGEST = "0"
            GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "0"
            GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
            GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
        }
    },
    @{ Name = "async-only"; Env = @{
            GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
            GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "1"
            GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "0"
            GITGRAPH_INDEX_FAST_MODE = "0"
            GITGRAPH_LEVEL3_PACK_AWARE_INGEST = "0"
            GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "0"
            GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
            GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
        }
    },
    @{ Name = "async+rename-off"; Env = @{
            GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
            GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "0"
            GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "0"
            GITGRAPH_INDEX_FAST_MODE = "0"
            GITGRAPH_LEVEL3_PACK_AWARE_INGEST = "0"
            GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "0"
            GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
            GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
        }
    },
    @{ Name = "async+memoized-diff"; Env = @{
            GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
            GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "0"
            GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "1"
            GITGRAPH_INDEX_FAST_MODE = "0"
            GITGRAPH_LEVEL3_PACK_AWARE_INGEST = "0"
            GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "1"
            GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
            GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
        }
    },
    @{ Name = "full-stack"; Env = @{
            GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
            GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "0"
            GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "1"
            GITGRAPH_INDEX_FAST_MODE = "1"
            GITGRAPH_LEVEL3_PACK_AWARE_INGEST = "1"
            GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "1"
            GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
            GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = "1024"
            GITGRAPH_FILE_CHANGE_PARALLEL_THREADS = "12"
        }
    }
)

$results = New-Object System.Collections.Generic.List[object]
$scenariosToRun = @()
if (-not [string]::IsNullOrWhiteSpace($OnlyScenario)) {
    $scenariosToRun = @($scenarios | Where-Object { $_.Name -eq $OnlyScenario })
    if ($scenariosToRun.Count -eq 0) {
        throw ("Unknown scenario requested via OnlyScenario: {0}" -f $OnlyScenario)
    }
} elseif (-not [string]::IsNullOrWhiteSpace($RequiredScenario)) {
    $scenariosToRun = @($scenarios | Where-Object { $_.Name -eq $RequiredScenario })
    if ($scenariosToRun.Count -eq 0) {
        throw ("Required scenario missing from configured scenarios: {0}" -f $RequiredScenario)
    }
} else {
    $scenariosToRun = $scenarios
}

foreach ($scenario in $scenariosToRun) {
    Write-Host ("Running ablation scenario: {0}" -f $scenario.Name)
    $results.Add((Invoke-Scenario -ScenarioName $scenario.Name -EnvVars $scenario.Env))
}

$thresholdError = $null
if ($MinAggregateCps -ge 0) {
    if ([string]::IsNullOrWhiteSpace($RequiredScenario)) {
        throw "MinAggregateCps requires RequiredScenario."
    }
    $required = $results | Where-Object { $_.Scenario -eq $RequiredScenario } | Select-Object -First 1
    if ($null -eq $required) {
        throw ("Required scenario missing from run results: {0}" -f $RequiredScenario)
    }
    if ([double]$required.AggregateCpsMedian -lt $MinAggregateCps) {
        $thresholdError = ("Ablation threshold failed: scenario '{0}' aggregate CPS median {1} < min {2}" -f `
            $RequiredScenario, $required.AggregateCpsMedian, $MinAggregateCps)
    }
}

$lines = @(
    "# Perf Ablation (25k workload)",
    "",
    ("Runs per scenario: {0}" -f $Runs),
    "",
    "| Scenario | Aggregate CPS (median) | Full CPS (median) | Core End-to-End ms (median) | Bottleneck Phase | Bottleneck ms (median) | Walk Scope (mode) | Walk Tip Count (median) | Pruned Parent Expansions (median) |",
    "| --- | ---: | ---: | ---: | --- | ---: | --- | ---: | ---: |"
)
foreach ($result in $results) {
    $lines += ("| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} |" -f `
        $result.Scenario, `
        $result.AggregateCpsMedian, `
        $result.FullCpsMedian, `
        $result.CoreEndToEndMsMedian, `
        $result.BottleneckPhase, `
        $result.BottleneckMsMedian, `
        $result.WalkScopeMode, `
        $result.AggregateWalkTipCountMedian, `
        $result.AggregatePrunedParentExpansionsMedian)
}
$lines += ""
$lines += "Raw aggregate CPS values (per scenario, per run):"
foreach ($result in $results) {
    $lines += ("- {0}: {1}" -f $result.Scenario, $result.AggregateCpsRaw)
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = "tmp/perf-ablation-25k.md"
}
if (-not (Test-Path (Split-Path -Parent $OutputPath))) {
    New-Item -Path (Split-Path -Parent $OutputPath) -ItemType Directory -Force | Out-Null
}
$lines -join [Environment]::NewLine | Set-Content -Path $OutputPath -Encoding UTF8
Write-Host ("Ablation report written: {0}" -f $OutputPath)

if ($null -ne $thresholdError) {
    throw $thresholdError
}
