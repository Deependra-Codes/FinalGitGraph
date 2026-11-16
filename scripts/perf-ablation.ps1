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
    [double]$MinAggregateCps = -1,
    [double]$MinFullCps = -1,
    [switch]$RequireQueryPositive,
    [double]$MinCpsGainVsControlPercent = 15,
    [switch]$CommitWalkResearch90k,
    [switch]$Resume,
    [int]$CooldownSeconds = -1,
    [int]$CooldownBetweenRunsSeconds = -1,
    [double]$CooldownCpuIdleThreshold = 15.0,
    [double]$CooldownProcPerfMinPercent = 80.0,
    [int]$CooldownMaxWaitSeconds = 180,
    [switch]$NoCooldown
)

$ErrorActionPreference = "Stop"

if ($CommitWalkResearch90k) {
    if ($Cycles -eq 2500) { $Cycles = 9000 }
    if ($IncrementalCommits -eq 2500) { $IncrementalCommits = 9000 }
    if ($WindowLimit -eq 500) { $WindowLimit = 1000 }
    if ($MaxNodes -eq 25000) { $MaxNodes = 90000 }
}

if ($NoCooldown) {
    $effectiveCooldownScenario = 0
    $effectiveCooldownRun = 0
} else {
    $effectiveCooldownScenario = if ($CooldownSeconds -ge 0) {
        $CooldownSeconds
    } elseif ($CommitWalkResearch90k) {
        30
    } else {
        10
    }
    $effectiveCooldownRun = if ($CooldownBetweenRunsSeconds -ge 0) {
        $CooldownBetweenRunsSeconds
    } elseif ($CommitWalkResearch90k) {
        15
    } else {
        5
    }
}
$cooldownThrottleEvents = New-Object System.Collections.Generic.List[string]

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

function Get-CpuUsagePercent {
    try {
        $sample = (Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 1 -ErrorAction Stop).CounterSamples[0].CookedValue
        return [Math]::Round($sample, 1)
    }
    catch {
        return -1.0
    }
}

function Get-ProcessorPerformancePercent {
    try {
        $sample = (Get-Counter '\Processor Information(_Total)\% Processor Performance' -SampleInterval 1 -MaxSamples 1 -ErrorAction Stop).CounterSamples[0].CookedValue
        return [Math]::Round($sample, 1)
    }
    catch {
        return -1.0
    }
}

function Wait-ForCpuCooldown {
    param(
        [int]$FixedSleepSeconds,
        [string]$PhaseLabel
    )

    if ($NoCooldown -or $FixedSleepSeconds -le 0) {
        return
    }

    Write-Host ("[{0}] cooldown: sleeping {1}s ({2})" -f (Get-Date -Format "HH:mm:ss"), $FixedSleepSeconds, $PhaseLabel)
    Start-Sleep -Seconds $FixedSleepSeconds

    $waitedExtra = 0
    $pollInterval = 3
    $maxExtra = $CooldownMaxWaitSeconds

    while ($waitedExtra -lt $maxExtra) {
        $cpuUsage = Get-CpuUsagePercent
        $procPerf = Get-ProcessorPerformancePercent

        $cpuIdle = ($cpuUsage -ge 0 -and $cpuUsage -lt $CooldownCpuIdleThreshold)
        $procOk = ($procPerf -lt 0 -or $procPerf -ge $CooldownProcPerfMinPercent)

        if ($cpuIdle -and $procOk) {
            if ($waitedExtra -gt 0) {
                Write-Host ("[{0}] cooldown: settled after {1}s extra wait (cpu={2}%, perf={3}%)" -f `
                    (Get-Date -Format "HH:mm:ss"), $waitedExtra, $cpuUsage, $procPerf)
            }
            return
        }

        $reason = ""
        if (-not $cpuIdle -and $cpuUsage -ge 0) {
            $reason += ("cpu={0}%>{1}%" -f $cpuUsage, $CooldownCpuIdleThreshold)
        }
        if (-not $procOk -and $procPerf -ge 0) {
            if ($reason.Length -gt 0) { $reason += ", " }
            $reason += ("proc_perf={0}%<{1}%" -f $procPerf, $CooldownProcPerfMinPercent)
        }

        if ($waitedExtra -eq 0 -or ($waitedExtra % 15) -eq 0) {
            Write-Host ("[{0}] cooldown: waiting for system to settle ({1}) [{2}]" -f `
                (Get-Date -Format "HH:mm:ss"), $reason, $PhaseLabel)
        }

        Start-Sleep -Seconds $pollInterval
        $waitedExtra += $pollInterval
    }

    $cpuUsage = Get-CpuUsagePercent
    $procPerf = Get-ProcessorPerformancePercent
    $throttleMsg = ("[{0}] cooldown: max wait {1}s reached, proceeding (cpu={2}%, perf={3}%) [{4}]" -f `
        (Get-Date -Format "HH:mm:ss"), $maxExtra, $cpuUsage, $procPerf, $PhaseLabel)
    Write-Host $throttleMsg
    $cooldownThrottleEvents.Add($throttleMsg)
}

function Merge-EnvMaps {
    param([hashtable[]]$Maps)
    $merged = @{}
    foreach ($map in $Maps) {
        if ($null -eq $map) {
            continue
        }
        foreach ($entry in $map.GetEnumerator()) {
            $merged[$entry.Key] = $entry.Value
        }
    }
    return $merged
}

function Get-ScenarioArtifactDirName {
    param([string]$ScenarioName)
    $sanitized = ($ScenarioName -replace '[^a-zA-Z0-9_-]', '-')
    if ($sanitized.Length -gt 40) {
        $sanitized = $sanitized.Substring(0, 40)
    }

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($ScenarioName)
    $hashBytes = [System.Security.Cryptography.SHA1]::Create().ComputeHash($bytes)
    $hash = [System.BitConverter]::ToString($hashBytes).Replace("-", "").ToLowerInvariant()
    $shortHash = $hash.Substring(0, 10)
    return ("{0}-{1}" -f $sanitized, $shortHash)
}

function Parse-ScenarioRunMetrics {
    param([string]$ReportPath)
    return @{
        AggregateCps = (Parse-Double (Get-ReportValue -Path $ReportPath -Key "aggregate_commit_throughput_per_sec"))
        FullCps = (Parse-Double (Get-ReportValue -Path $ReportPath -Key "full_commit_throughput_per_sec"))
        CoreMs = (Parse-Double (Get-ReportValue -Path $ReportPath -Key "core_end_to_end"))
        QueryBenchmarkMs = (Parse-Double (Get-ReportValueOrDefault -Path $ReportPath -Key "query_benchmark" -DefaultValue "0"))
        InMemoryQueryBenchmarkMs = (Parse-Double (Get-ReportValueOrDefault -Path $ReportPath -Key "in_memory_query_benchmark" -DefaultValue "0"))
        BottleneckPhase = (Get-ReportValue -Path $ReportPath -Key "aggregate_commit_bottleneck_phase")
        BottleneckMs = (Parse-Double (Get-ReportValue -Path $ReportPath -Key "aggregate_commit_bottleneck_phase_ms"))
        WalkScope = (Get-ReportValueOrDefault -Path $ReportPath -Key "full_commit_walk_scope" -DefaultValue "n/a")
        WalkTipCount = (Parse-Double (Get-ReportValueOrDefault -Path $ReportPath -Key "aggregate_commit_walk_tip_count" -DefaultValue "0"))
        PrunedParentExpansions = (Parse-Double (Get-ReportValueOrDefault -Path $ReportPath -Key "aggregate_commit_walk_pruned_parent_expansions" -DefaultValue "0"))
        CommitWalkMs = (Parse-Double (Get-ReportValueOrDefault -Path $ReportPath -Key "ingest_commit_walk_ms" -DefaultValue "0"))
        WalkEngine = (Get-ReportValueOrDefault -Path $ReportPath -Key "full_commit_walk_engine" -DefaultValue "n/a")
        WalkSort = (Get-ReportValueOrDefault -Path $ReportPath -Key "full_commit_walk_sort_effective" -DefaultValue "n/a")
    }
}

function Invoke-Scenario {
    param(
        [string]$ScenarioName,
        [hashtable]$EnvVars
    )

    $scenarioDir = Join-Path "tmp/perf-ablation" (Get-ScenarioArtifactDirName -ScenarioName $ScenarioName)
    New-Item -Path $scenarioDir -ItemType Directory -Force | Out-Null

    $aggregateCps = New-Object System.Collections.Generic.List[double]
    $fullCps = New-Object System.Collections.Generic.List[double]
    $coreMs = New-Object System.Collections.Generic.List[double]
    $queryBenchmarkMs = New-Object System.Collections.Generic.List[double]
    $inMemoryQueryBenchmarkMs = New-Object System.Collections.Generic.List[double]
    $bottleneckPhase = New-Object System.Collections.Generic.List[string]
    $bottleneckMs = New-Object System.Collections.Generic.List[double]
    $walkScope = New-Object System.Collections.Generic.List[string]
    $walkTipCount = New-Object System.Collections.Generic.List[double]
    $prunedParentExpansions = New-Object System.Collections.Generic.List[double]
    $commitWalkMs = New-Object System.Collections.Generic.List[double]
    $walkEngine = New-Object System.Collections.Generic.List[string]
    $walkSort = New-Object System.Collections.Generic.List[string]

    $allKeys = @(
        "GITGRAPH_FILE_CHANGE_SYNC_STRICT",
        "GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH",
        "GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO",
        "GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER",
        "GITGRAPH_COMMIT_WALK_SORT",
        "GITGRAPH_COMMIT_WALK_SCOPE",
        "GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE",
        "GITGRAPH_FILE_CHANGE_PARALLEL_THREADS",
        "GITGRAPH_COMMIT_INGEST_PIPELINED",
        "GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE",
        "GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY",
        "GITGRAPH_GRAPH_OVERLAY_MAX_COMMITS",
        "GITGRAPH_GRAPH_OVERLAY_MAX_EDGES",
        "GITGRAPH_COMMIT_WINDOW_PREWARM",
        "GITGRAPH_COMMIT_WINDOW_PREWARM_LIMIT",
        "GITGRAPH_PERF_REPORT_DOCKER_ONLY"
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
        $failureReason = $null
        for ($run = 1; $run -le $Runs; $run++) {
            $reportPath = Join-Path $scenarioDir ("run-{0}.txt" -f $run)
            $reportPathForPerf = $reportPath -replace "\\", "/"
            $metrics = $null
            if ($Resume -and (Test-Path $reportPath)) {
                try {
                    $metrics = Parse-ScenarioRunMetrics -ReportPath $reportPath
                    Write-Host ("[{0}] {1} run {2}/{3} (resume cache hit)" -f (Get-Date -Format "HH:mm:ss"), $ScenarioName, $run, $Runs)
                }
                catch {
                    Write-Host ("[{0}] {1} run {2}/{3} (resume artifact invalid, re-running)" -f (Get-Date -Format "HH:mm:ss"), $ScenarioName, $run, $Runs)
                    $metrics = $null
                }
            }

            if ($null -eq $metrics) {
                if ($run -gt 1) {
                    Wait-ForCpuCooldown -FixedSleepSeconds $effectiveCooldownRun -PhaseLabel ("between-runs {0} run {1}" -f $ScenarioName, $run)
                }
                Write-Host ("[{0}] {1} run {2}/{3}" -f (Get-Date -Format "HH:mm:ss"), $ScenarioName, $run, $Runs)
                if ($CommitWalkResearch90k) {
                    & powershell -ExecutionPolicy Bypass -File "$PSScriptRoot\perf-report.ps1" `
                        -Cycles $Cycles `
                        -FeatureCommits $FeatureCommits `
                        -IncrementalCommits $IncrementalCommits `
                        -QueryRuns $QueryRuns `
                        -WindowLimit $WindowLimit `
                        -MaxNodes $MaxNodes `
                        -PreconditionCommitGraph `
                        -OutputPath $reportPathForPerf
                } else {
                    & powershell -ExecutionPolicy Bypass -File "$PSScriptRoot\perf-report.ps1" `
                        -Cycles $Cycles `
                        -FeatureCommits $FeatureCommits `
                        -IncrementalCommits $IncrementalCommits `
                        -QueryRuns $QueryRuns `
                        -WindowLimit $WindowLimit `
                        -MaxNodes $MaxNodes `
                        -OutputPath $reportPathForPerf
                }
                if ($LASTEXITCODE -ne 0) {
                    $failureReason = "perf-report failed for run $run"
                    break
                }
                if (-not (Test-Path $reportPath)) {
                    $failureReason = "perf-report output missing for run $run"
                    break
                }

                try {
                    $metrics = Parse-ScenarioRunMetrics -ReportPath $reportPath
                }
                catch {
                    $failureReason = "report parse failed for run ${run}: $($_.Exception.Message)"
                    break
                }
            }

            $aggregateCps.Add($metrics.AggregateCps)
            $fullCps.Add($metrics.FullCps)
            $coreMs.Add($metrics.CoreMs)
            $queryBenchmarkMs.Add($metrics.QueryBenchmarkMs)
            $inMemoryQueryBenchmarkMs.Add($metrics.InMemoryQueryBenchmarkMs)
            $bottleneckPhase.Add($metrics.BottleneckPhase)
            $bottleneckMs.Add($metrics.BottleneckMs)
            $walkScope.Add($metrics.WalkScope)
            $walkTipCount.Add($metrics.WalkTipCount)
            $prunedParentExpansions.Add($metrics.PrunedParentExpansions)
            $commitWalkMs.Add($metrics.CommitWalkMs)
            $walkEngine.Add($metrics.WalkEngine)
            $walkSort.Add($metrics.WalkSort)
        }

        if ($null -ne $failureReason) {
            return @{
                Scenario = $ScenarioName
                Failed = $true
                FailureReason = $failureReason
                AggregateCpsMedian = 0.0
                FullCpsMedian = 0.0
                CoreEndToEndMsMedian = 0.0
                QueryBenchmarkMedian = 0.0
                InMemoryQueryBenchmarkMedian = 0.0
                BottleneckPhase = "failed"
                BottleneckMsMedian = 0.0
                WalkScopeMode = "failed"
                WalkEngineMode = "failed"
                WalkSortMode = "failed"
                AggregateWalkTipCountMedian = 0.0
                AggregatePrunedParentExpansionsMedian = 0.0
                CommitWalkMsMedian = 0.0
                AggregateCpsRaw = "failed"
                QueryBenchmarkRaw = "failed"
                InMemoryQueryBenchmarkRaw = "failed"
            }
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
        Failed = $false
        FailureReason = ""
        AggregateCpsMedian = [Math]::Round((Median-Double $aggregateCps), 3)
        FullCpsMedian = [Math]::Round((Median-Double $fullCps), 3)
        CoreEndToEndMsMedian = [Math]::Round((Median-Double $coreMs), 3)
        QueryBenchmarkMedian = [Math]::Round((Median-Double $queryBenchmarkMs), 3)
        InMemoryQueryBenchmarkMedian = [Math]::Round((Median-Double $inMemoryQueryBenchmarkMs), 3)
        BottleneckPhase = Mode-String $bottleneckPhase
        BottleneckMsMedian = [Math]::Round((Median-Double $bottleneckMs), 3)
        WalkScopeMode = Mode-String $walkScope
        WalkEngineMode = Mode-String $walkEngine
        WalkSortMode = Mode-String $walkSort
        AggregateWalkTipCountMedian = [Math]::Round((Median-Double $walkTipCount), 3)
        AggregatePrunedParentExpansionsMedian = [Math]::Round((Median-Double $prunedParentExpansions), 3)
        CommitWalkMsMedian = [Math]::Round((Median-Double $commitWalkMs), 3)
        AggregateCpsRaw = (($aggregateCps | ForEach-Object { Format-DoubleInvariant $_ }) -join ", ")
        QueryBenchmarkRaw = (($queryBenchmarkMs | ForEach-Object { Format-DoubleInvariant $_ }) -join ", ")
        InMemoryQueryBenchmarkRaw = (($inMemoryQueryBenchmarkMs | ForEach-Object { Format-DoubleInvariant $_ }) -join ", ")
    }
}

$scenarios = @()
if ($CommitWalkResearch90k) {
    $baseResearchEnv = @{
        GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
        GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "0"
        GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "1"
        GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
        GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
        GITGRAPH_COMMIT_INGEST_PIPELINED = "1"
        GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE = "1"
        GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY = "32768"
        GITGRAPH_GRAPH_OVERLAY_MAX_COMMITS = "20000"
        GITGRAPH_GRAPH_OVERLAY_MAX_EDGES = "80000"
        GITGRAPH_COMMIT_WINDOW_PREWARM = "0"
        GITGRAPH_COMMIT_WINDOW_PREWARM_LIMIT = "1000"
        GITGRAPH_PERF_REPORT_DOCKER_ONLY = "1"
    }

    $controlEnv = Merge-EnvMaps @($baseResearchEnv)
    $scenarios += @{ Name = "control"; Env = $controlEnv }

    $pipelineLanes = @(
        @{ Name = "off"; Value = "0" },
        @{ Name = "on"; Value = "1" }
    )
    $adaptiveLanes = @(
        @{ Name = "off"; Value = "0" },
        @{ Name = "on"; Value = "1" }
    )
    $queueLanes = @(
        @{ Name = "auto"; Value = "" },
        @{ Name = "32768"; Value = "32768" },
        @{ Name = "65536"; Value = "65536" }
    )
    $overlayLanes = @(
        @{ Name = "aggressive"; Commits = "20000"; Edges = "80000" },
        @{ Name = "default"; Commits = ""; Edges = "" },
        @{ Name = "relaxed"; Commits = "200000"; Edges = "800000" }
    )
    $prewarmLanes = @(
        @{ Name = "off"; Value = "0" },
        @{ Name = "on"; Value = "1" }
    )

    foreach ($pipeline in $pipelineLanes) {
        foreach ($adaptive in $adaptiveLanes) {
            foreach ($queue in $queueLanes) {
                foreach ($overlay in $overlayLanes) {
                    foreach ($prewarm in $prewarmLanes) {
                        $envVars = Merge-EnvMaps @($baseResearchEnv)
                        $envVars["GITGRAPH_COMMIT_INGEST_PIPELINED"] = $pipeline.Value
                        $envVars["GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE"] = $adaptive.Value
                        $envVars["GITGRAPH_COMMIT_WINDOW_PREWARM"] = $prewarm.Value
                        if ([string]::IsNullOrWhiteSpace($queue.Value)) {
                            $envVars["GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY"] = ""
                        } else {
                            $envVars["GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY"] = $queue.Value
                        }
                        $envVars["GITGRAPH_GRAPH_OVERLAY_MAX_COMMITS"] = $overlay.Commits
                        $envVars["GITGRAPH_GRAPH_OVERLAY_MAX_EDGES"] = $overlay.Edges

                        $scenarioName = ("research-p={0}-a={1}-q={2}-o={3}-w={4}" -f `
                            $pipeline.Name, $adaptive.Name, $queue.Name, $overlay.Name, $prewarm.Name)
                        $scenarios += @{ Name = $scenarioName; Env = $envVars }
                    }
                }
            }
        }
    }
} else {
    $scenarios = @(
        @{ Name = "control"; Env = @{
                GITGRAPH_FILE_CHANGE_SYNC_STRICT = "1"
                GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "1"
                GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "0"
                GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "0"
                GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
                GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
                GITGRAPH_COMMIT_INGEST_PIPELINED = "0"
                GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE = "0"
                GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = "1024"
            }
        },
        @{ Name = "baseline-index-only"; Env = @{
                GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
                GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "1"
                GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "0"
                GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "0"
                GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
                GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
                GITGRAPH_COMMIT_INGEST_PIPELINED = "0"
                GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE = "0"
                GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = "8192"
            }
        },
        @{ Name = "adaptive-wave"; Env = @{
                GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
                GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "0"
                GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "0"
                GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "0"
                GITGRAPH_COMMIT_WALK_SORT = "commit_time_newest"
                GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
                GITGRAPH_COMMIT_INGEST_PIPELINED = "0"
                GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE = "1"
                GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = "8192"
            }
        },
        @{ Name = "full-decoupled"; Env = @{
                GITGRAPH_FILE_CHANGE_SYNC_STRICT = "0"
                GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH = "0"
                GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO = "1"
                GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER = "1"
                GITGRAPH_COMMIT_WALK_SCOPE = "frontier"
                GITGRAPH_COMMIT_INGEST_PIPELINED = "1"
                GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE = "1"
                GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE = "16384"
                GITGRAPH_FILE_CHANGE_PARALLEL_THREADS = "12"
                GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY = "256"
            }
        }
    )
}

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

$scenarioIndex = 0
foreach ($scenario in $scenariosToRun) {
    if ($scenarioIndex -gt 0) {
        Wait-ForCpuCooldown -FixedSleepSeconds $effectiveCooldownScenario -PhaseLabel ("before-scenario {0}" -f $scenario.Name)
    }
    Write-Host ("Running ablation scenario: {0} [{1}/{2}]" -f $scenario.Name, ($scenarioIndex + 1), $scenariosToRun.Count)
    $results.Add((Invoke-Scenario -ScenarioName $scenario.Name -EnvVars $scenario.Env))
    $scenarioIndex++
}

$thresholdError = $null
if ($MinAggregateCps -ge 0 -or $MinFullCps -ge 0 -or $RequireQueryPositive) {
    if ([string]::IsNullOrWhiteSpace($RequiredScenario)) {
        throw "MinAggregateCps/MinFullCps/RequireQueryPositive require RequiredScenario."
    }
    $required = $results | Where-Object { $_.Scenario -eq $RequiredScenario } | Select-Object -First 1
    if ($null -eq $required) {
        throw ("Required scenario missing from run results: {0}" -f $RequiredScenario)
    }
    if ($required.Failed) {
        $thresholdError = ("Ablation threshold failed: scenario '{0}' failed ({1})" -f `
            $RequiredScenario, $required.FailureReason)
    } elseif ($MinAggregateCps -ge 0 -and [double]$required.AggregateCpsMedian -lt $MinAggregateCps) {
        $thresholdError = ("Ablation threshold failed: scenario '{0}' aggregate CPS median {1} < min {2}" -f `
            $RequiredScenario, $required.AggregateCpsMedian, $MinAggregateCps)
    } elseif ($MinFullCps -ge 0 -and [double]$required.FullCpsMedian -lt $MinFullCps) {
        $thresholdError = ("Ablation threshold failed: scenario '{0}' full CPS median {1} < min {2}" -f `
            $RequiredScenario, $required.FullCpsMedian, $MinFullCps)
    } elseif ($RequireQueryPositive) {
        $control = $results | Where-Object { $_.Scenario -eq "control" } | Select-Object -First 1
        if ($null -eq $control) {
            throw "RequireQueryPositive needs control scenario in results."
        }
        $queryDelta = [double]$control.QueryBenchmarkMedian - [double]$required.QueryBenchmarkMedian
        $inMemoryQueryDelta = [double]$control.InMemoryQueryBenchmarkMedian - [double]$required.InMemoryQueryBenchmarkMedian
        if ($queryDelta -le 0 -or $inMemoryQueryDelta -le 0) {
            $thresholdError = ("Ablation threshold failed: scenario '{0}' is not query-positive vs control (query_delta_ms={1}, in_memory_query_delta_ms={2})" -f `
                $RequiredScenario, `
                (Format-DoubleInvariant $queryDelta), `
                (Format-DoubleInvariant $inMemoryQueryDelta))
        }
    }
}

$workloadLabel = if ($CommitWalkResearch90k) {
    "90k workload (csr-incremental research matrix)"
} else {
    "25k workload"
}

$cooldownLabel = if ($NoCooldown) {
    "disabled"
} else {
    ("scenario={0}s, run={1}s, cpu_idle<{2}%, proc_perf>={3}%, max_wait={4}s" -f `
        $effectiveCooldownScenario, $effectiveCooldownRun, `
        $CooldownCpuIdleThreshold, $CooldownProcPerfMinPercent, $CooldownMaxWaitSeconds)
}

$lines = @(
    ("# Perf Ablation ({0})" -f $workloadLabel),
    "",
    ("Runs per scenario: {0}" -f $Runs),
    ("Thermal cooldown: {0}" -f $cooldownLabel),
    "",
    "| Scenario | Aggregate CPS (median) | Full CPS (median) | Query Benchmark ms (median) | In-Memory Query ms (median) | Core End-to-End ms (median) | Bottleneck Phase | Bottleneck ms (median) | Commit Walk ms (median) | Walk Engine (mode) | Walk Sort (mode) | Walk Scope (mode) | Walk Tip Count (median) | Pruned Parent Expansions (median) |",
    "| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | --- | --- | ---: | ---: |"
)
foreach ($result in $results) {
    if ($result.Failed) {
        $lines += ("| {0} | n/a | n/a | n/a | n/a | n/a | failed ({1}) | n/a | n/a | n/a | n/a | n/a | n/a | n/a |" -f `
            $result.Scenario, `
            $result.FailureReason)
    } else {
        $lines += ("| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} | {10} | {11} | {12} | {13} |" -f `
            $result.Scenario, `
            $result.AggregateCpsMedian, `
            $result.FullCpsMedian, `
            $result.QueryBenchmarkMedian, `
            $result.InMemoryQueryBenchmarkMedian, `
            $result.CoreEndToEndMsMedian, `
            $result.BottleneckPhase, `
            $result.BottleneckMsMedian, `
            $result.CommitWalkMsMedian, `
            $result.WalkEngineMode, `
            $result.WalkSortMode, `
            $result.WalkScopeMode, `
            $result.AggregateWalkTipCountMedian, `
            $result.AggregatePrunedParentExpansionsMedian)
    }
}

$lines += ""
$lines += "Raw aggregate CPS values (per scenario, per run):"
foreach ($result in $results) {
    if ($result.Failed) {
        $lines += ("- {0}: failed ({1})" -f $result.Scenario, $result.FailureReason)
    } else {
        $lines += ("- {0}: {1}" -f $result.Scenario, $result.AggregateCpsRaw)
    }
}
$lines += ""
$lines += "Raw query_benchmark values (per scenario, per run):"
foreach ($result in $results) {
    if ($result.Failed) {
        $lines += ("- {0}: failed ({1})" -f $result.Scenario, $result.FailureReason)
    } else {
        $lines += ("- {0}: {1}" -f $result.Scenario, $result.QueryBenchmarkRaw)
    }
}
$lines += ""
$lines += "Raw in_memory_query_benchmark values (per scenario, per run):"
foreach ($result in $results) {
    if ($result.Failed) {
        $lines += ("- {0}: failed ({1})" -f $result.Scenario, $result.FailureReason)
    } else {
        $lines += ("- {0}: {1}" -f $result.Scenario, $result.InMemoryQueryBenchmarkRaw)
    }
}

if ($CommitWalkResearch90k) {
    $control = $results | Where-Object { $_.Scenario -eq "control" } | Select-Object -First 1
    if ($null -eq $control -or $control.Failed) {
        $lines += ""
        $lines += "## Approach Leaderboard (Skipped)"
        if ($null -eq $control) {
            $lines += "- control scenario is required for delta ranking but was not present in this run."
        } else {
            $lines += ("- control scenario failed, so delta ranking was skipped ({0})." -f $control.FailureReason)
        }
    } else {
        $researchAggregateGate = 50000.0
        $researchFullGate = 100000.0

        $failedResearch = @($results | Where-Object { $_.Scenario -ne "control" -and $_.Failed })
        $ranked = @($results | Where-Object { $_.Scenario -ne "control" -and -not $_.Failed } | Sort-Object -Property AggregateCpsMedian -Descending)
        $promotionEligible = New-Object System.Collections.Generic.List[object]
        foreach ($entry in $ranked) {
            $deltaCps = [double]$entry.AggregateCpsMedian - [double]$control.AggregateCpsMedian
            $deltaCpsPct = if ([double]$control.AggregateCpsMedian -gt 0) {
                ($deltaCps / [double]$control.AggregateCpsMedian) * 100.0
            } else {
                0.0
            }
            $queryDeltaMs = [double]$control.QueryBenchmarkMedian - [double]$entry.QueryBenchmarkMedian
            $inMemoryQueryDeltaMs = [double]$control.InMemoryQueryBenchmarkMedian - [double]$entry.InMemoryQueryBenchmarkMedian
            $entryEligible = (
                $deltaCpsPct -ge $MinCpsGainVsControlPercent -and
                $queryDeltaMs -gt 0 -and
                $inMemoryQueryDeltaMs -gt 0 -and
                [double]$entry.AggregateCpsMedian -ge $researchAggregateGate -and
                [double]$entry.FullCpsMedian -ge $researchFullGate
            )
            if ($entryEligible) {
                $promotionEligible.Add($entry)
            }
        }
        $accepted = @($promotionEligible | Select-Object -First 2)
        $acceptedSet = @{}
        foreach ($entry in $accepted) {
            $acceptedSet[$entry.Scenario] = $true
        }

        $lines += ""
        $lines += "## Approach Leaderboard (Ranked)"
        $lines += ""
        $lines += "| Rank | Scenario | Aggregate CPS (median) | Delta CPS vs control | Delta CPS % vs control | Commit Walk ms (median) | Commit Walk improvement ms | Query delta ms vs control | In-Memory query delta ms vs control | Bottleneck Phase | Verdict |"
        $lines += "| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |"
        $rank = 1
        foreach ($entry in $ranked) {
            $deltaCps = [double]$entry.AggregateCpsMedian - [double]$control.AggregateCpsMedian
            $deltaCpsPct = if ([double]$control.AggregateCpsMedian -gt 0) {
                ($deltaCps / [double]$control.AggregateCpsMedian) * 100.0
            } else {
                0.0
            }
            $walkImprovementMs = [double]$control.CommitWalkMsMedian - [double]$entry.CommitWalkMsMedian
            $queryDeltaMs = [double]$control.QueryBenchmarkMedian - [double]$entry.QueryBenchmarkMedian
            $inMemoryQueryDeltaMs = [double]$control.InMemoryQueryBenchmarkMedian - [double]$entry.InMemoryQueryBenchmarkMedian
            $verdict = if ($acceptedSet.ContainsKey($entry.Scenario)) {
                "accepted-promotable-top2"
            } else {
                "rejected"
            }
            $lines += ("| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} | {10} |" -f `
                $rank, `
                $entry.Scenario, `
                (Format-DoubleInvariant $entry.AggregateCpsMedian), `
                (Format-DoubleInvariant $deltaCps), `
                (Format-DoubleInvariant $deltaCpsPct), `
                (Format-DoubleInvariant $entry.CommitWalkMsMedian), `
                (Format-DoubleInvariant $walkImprovementMs), `
                (Format-DoubleInvariant $queryDeltaMs), `
                (Format-DoubleInvariant $inMemoryQueryDeltaMs), `
                $entry.BottleneckPhase, `
                $verdict)
            $rank = $rank + 1
        }

        $lines += ""
        $lines += "## Accepted Approaches (Top 2 Promotable)"
        if ($accepted.Count -eq 0) {
            $lines += "- none: no scenario met promotion gates"
        } else {
            foreach ($entry in $accepted) {
                $deltaCps = [double]$entry.AggregateCpsMedian - [double]$control.AggregateCpsMedian
                $deltaCpsPct = if ([double]$control.AggregateCpsMedian -gt 0) {
                    ($deltaCps / [double]$control.AggregateCpsMedian) * 100.0
                } else {
                    0.0
                }
                $queryDeltaMs = [double]$control.QueryBenchmarkMedian - [double]$entry.QueryBenchmarkMedian
                $inMemoryQueryDeltaMs = [double]$control.InMemoryQueryBenchmarkMedian - [double]$entry.InMemoryQueryBenchmarkMedian
                $lines += ("- {0}: accepted (delta_cps_pct={1}, query_delta_ms={2}, in_memory_query_delta_ms={3})" -f `
                    $entry.Scenario, `
                    (Format-DoubleInvariant $deltaCpsPct), `
                    (Format-DoubleInvariant $queryDeltaMs), `
                    (Format-DoubleInvariant $inMemoryQueryDeltaMs))
            }
        }

        $lines += ""
        $lines += "## Rejected Approaches (Measured Reason)"
        foreach ($entry in $ranked) {
            if ($acceptedSet.ContainsKey($entry.Scenario)) {
                continue
            }
            $deltaCps = [double]$entry.AggregateCpsMedian - [double]$control.AggregateCpsMedian
            $deltaCpsPct = if ([double]$control.AggregateCpsMedian -gt 0) {
                ($deltaCps / [double]$control.AggregateCpsMedian) * 100.0
            } else {
                0.0
            }
            $queryDeltaMs = [double]$control.QueryBenchmarkMedian - [double]$entry.QueryBenchmarkMedian
            $inMemoryQueryDeltaMs = [double]$control.InMemoryQueryBenchmarkMedian - [double]$entry.InMemoryQueryBenchmarkMedian
            $reason = if ($deltaCpsPct -lt $MinCpsGainVsControlPercent) {
                "cps gain below promotion threshold"
            } elseif ([double]$entry.AggregateCpsMedian -lt $researchAggregateGate) {
                "aggregate CPS below 50k gate"
            } elseif ([double]$entry.FullCpsMedian -lt $researchFullGate) {
                "full CPS below 100k gate"
            } elseif ($queryDeltaMs -le 0) {
                "query_benchmark not improved vs control"
            } elseif ($inMemoryQueryDeltaMs -le 0) {
                "in_memory_query_benchmark not improved vs control"
            } else {
                "outside top-2 promotable ranking"
            }
            $lines += ("- {0}: rejected ({1}; delta_cps_pct={2}; query_delta_ms={3}; in_memory_query_delta_ms={4})" -f `
                $entry.Scenario, `
                $reason, `
                (Format-DoubleInvariant $deltaCpsPct), `
                (Format-DoubleInvariant $queryDeltaMs), `
                (Format-DoubleInvariant $inMemoryQueryDeltaMs))
        }

        if ($failedResearch.Count -gt 0) {
            $lines += ""
            $lines += "## Rejected Approaches (Execution Failure)"
            foreach ($entry in $failedResearch) {
                $lines += ("- {0}: rejected (scenario execution failed: {1})" -f `
                    $entry.Scenario, `
                    $entry.FailureReason)
            }
        }
    }
}

if ($cooldownThrottleEvents.Count -gt 0) {
    $lines += ""
    $lines += "## Thermal Throttle Events"
    $lines += ""
    $lines += ("**{0} event(s)** where the system did not settle within the max cooldown wait. Results from scenarios following these events may be thermally impacted." -f $cooldownThrottleEvents.Count)
    $lines += ""
    foreach ($evt in $cooldownThrottleEvents) {
        $lines += ("- {0}" -f $evt)
    }
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
