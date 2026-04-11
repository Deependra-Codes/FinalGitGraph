# platform: any
param(
    [string]$CargoTargetRoot = "",
    [int]$Cycles = 120,
    [int]$FeatureCommits = 4,
    [int]$IncrementalCommits = 40,
    [int]$QueryRuns = 25,
    [int]$WindowLimit = 200,
    [int]$MaxNodes = 2000,
    [string]$OutputPath = "tmp/perf-report-latest.txt",
    [string]$FixtureRoot = "",
    [switch]$WarmFixture,
    [switch]$PreconditionCommitGraph,
    [switch]$KeepFixture,
    [switch]$EnableLsmRocksdb,
    [string]$LibclangPath = "",
    [switch]$ProbeOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$repoRootPath = $repoRoot.Path
$cargoTargetRootWasExplicit = $PSBoundParameters.ContainsKey("CargoTargetRoot") -and `
    -not [string]::IsNullOrWhiteSpace($CargoTargetRoot)
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
    } else {
        $CargoTargetRoot = Join-Path $userHome ".cargo-target/gitgraph-lab"
    }
}
if (-not (Test-Path $CargoTargetRoot)) {
    New-Item -Path $CargoTargetRoot -ItemType Directory -Force | Out-Null
}
$env:CARGO_TARGET_DIR = $CargoTargetRoot

function Normalize-AbsolutePath {
    param([string]$PathValue)

    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return ""
    }

    $absolute = [System.IO.Path]::GetFullPath($PathValue)
    return $absolute.TrimEnd('\', '/')
}

function Set-CargoTargetRoot {
    param([string]$NextCargoTargetRoot)

    if ([string]::IsNullOrWhiteSpace($NextCargoTargetRoot)) {
        return
    }
    if (-not (Test-Path $NextCargoTargetRoot)) {
        New-Item -Path $NextCargoTargetRoot -ItemType Directory -Force | Out-Null
    }
    $script:CargoTargetRoot = $NextCargoTargetRoot
    $env:CARGO_TARGET_DIR = $NextCargoTargetRoot
}

function Format-ElapsedSeconds {
    param([TimeSpan]$Elapsed)
    return [Math]::Round($Elapsed.TotalSeconds, 3)
}

function Test-DockerReady {
    $dockerCommand = Get-Command "docker" -ErrorAction SilentlyContinue
    if ($null -eq $dockerCommand) {
        return $false
    }
    docker info --format "{{.ServerVersion}}" *> $null
    return $LASTEXITCODE -eq 0
}

function Test-IsPolicyBlocked {
    param([string]$OutputText)
    return $OutputText -match "Application Control policy has blocked this file" `
        -or $OutputText -match "os error 4551"
}

function Test-EnvEnabled {
    param([string]$Name)
    $raw = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($raw)) {
        return $false
    }
    $normalized = $raw.Trim().ToLowerInvariant()
    return $normalized -in @("1", "true", "yes", "on")
}

function Convert-ToBashSingleQuoted {
    param([string]$Value)
    $escaped = $Value -replace "'", "'`"`'`"`'"
    return "'" + $escaped + "'"
}

function Invoke-ExternalPhase {
    param(
        [string]$Executable,
        [string[]]$Arguments,
        [string]$PhaseLabel
    )

    Write-Host $PhaseLabel
    $phaseLog = New-TemporaryFile
    try {
        $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        $previousErrorActionPreference = $ErrorActionPreference
        $exitCode = 0
        $invocationError = $null
        try {
            $ErrorActionPreference = "Continue"
            try {
                & $Executable @Arguments 2>&1 | Tee-Object -FilePath $phaseLog.FullName
                $exitCode = $LASTEXITCODE
            }
            catch {
                $invocationError = $_.Exception.Message
                if (-not [string]::IsNullOrWhiteSpace($invocationError)) {
                    $invocationError | Tee-Object -FilePath $phaseLog.FullName -Append | Out-Null
                }
                $exitCode = 1
            }
        }
        finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        $stopwatch.Stop()

        $outputText = ""
        if (Test-Path $phaseLog.FullName) {
            $outputText = Get-Content -Path $phaseLog.FullName -Raw
        }

        return @{
            ExitCode   = $exitCode
            Duration   = $stopwatch.Elapsed
            OutputText = $outputText
        }
    }
    finally {
        Remove-Item -Path $phaseLog.FullName -ErrorAction SilentlyContinue
    }
}

function Get-DockerRunArgs {
    param(
        [string]$RepoRootPath,
        [string]$CommandText
    )

    $forwardedEnvArgs = @()
    $forwardedEnvKeys = @(
        "GITGRAPH_INDEX_FAST_MODE",
        "GITGRAPH_INGEST_WRITE_PROFILE",
        "GITGRAPH_COMMIT_WALK_SORT",
        "GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_FULL",
        "GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_DRAIN",
        "GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE",
        "GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY",
        "GITGRAPH_QUERY_RUNTIME_CACHE",
        "GITGRAPH_LEVEL3_STREAMING_CSR",
        "GITGRAPH_LEVEL3_STREAMING_CSR_MODE",
        "GITGRAPH_COMMIT_INGEST_PIPELINED",
        "GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE",
        "GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY",
        "GITGRAPH_GRAPH_OVERLAY_MAX_COMMITS",
        "GITGRAPH_GRAPH_OVERLAY_MAX_EDGES",
        "GITGRAPH_COMMIT_WINDOW_PREWARM",
        "GITGRAPH_COMMIT_WINDOW_PREWARM_LIMIT"
    )
    foreach ($key in $forwardedEnvKeys) {
        $value = [Environment]::GetEnvironmentVariable($key)
        if (-not [string]::IsNullOrWhiteSpace($value)) {
            $forwardedEnvArgs += @("-e", "${key}=${value}")
        }
    }

    $baseArgs = @(
        "run",
        "--rm",
        "-v",
        "${RepoRootPath}:/work",
        "-v",
        "gitgraph_cargo_registry:/usr/local/cargo/registry",
        "-v",
        "gitgraph_cargo_git:/usr/local/cargo/git",
        "-v",
        "gitgraph_rustup:/usr/local/rustup",
        "-v",
        "gitgraph_target:/cargo-target",
        "-e",
        "CARGO_TARGET_DIR=/cargo-target",
        "-e",
        "RUSTUP_HOME=/usr/local/rustup",
        "-e",
        "PATH=/usr/local/cargo/bin:/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    )
    $tailArgs = @(
        "-w",
        "/work",
        "rust:1.94",
        "bash",
        "-c",
        $CommandText
    )

    return $baseArgs + $forwardedEnvArgs + $tailArgs
}

function Invoke-DockerPhase {
    param(
        [string]$RepoRootPath,
        [string]$CommandText,
        [string]$PhaseLabel
    )

    $dockerArgs = Get-DockerRunArgs -RepoRootPath $RepoRootPath -CommandText $CommandText
    return Invoke-ExternalPhase -Executable "docker" -Arguments $dockerArgs -PhaseLabel $PhaseLabel
}

function Get-ReportArgs {
    $resolvedFixtureRoot = $FixtureRoot
    if ($WarmFixture -and [string]::IsNullOrWhiteSpace($resolvedFixtureRoot)) {
        $resolvedFixtureRoot = Join-Path $repoRootPath ("tmp/perf-fixtures/c{0}-f{1}-i{2}-n{3}" -f $Cycles, $FeatureCommits, $IncrementalCommits, $MaxNodes)
    }

    $args = @(
        "--cycles", "$Cycles",
        "--feature-commits", "$FeatureCommits",
        "--incremental-commits", "$IncrementalCommits",
        "--query-runs", "$QueryRuns",
        "--window-limit", "$WindowLimit",
        "--max-nodes", "$MaxNodes"
    )

    if (-not [string]::IsNullOrWhiteSpace($OutputPath)) {
        $args += @("--output", "$OutputPath")
    }
    if (-not [string]::IsNullOrWhiteSpace($resolvedFixtureRoot)) {
        $args += @("--fixture-root", "$resolvedFixtureRoot")
    }
    if ($WarmFixture) {
        $args += "--reuse-fixture"
    }
    if ($PreconditionCommitGraph) {
        $args += "--precondition-commit-graph"
    }
    if ($KeepFixture) {
        $args += "--keep-fixture"
    }
    return $args
}

function Get-CargoFeatureArgs {
    if ($EnableLsmRocksdb) {
        return @("--features", "lsm-rocksdb")
    }
    return @()
}

function Invoke-PerfReportDocker {
    param([string]$RepoRootPath)

    if ($EnableLsmRocksdb) {
        throw "Docker fallback is disabled for -EnableLsmRocksdb. Run on host with -LibclangPath and ensure LLVM is installed."
    }

    Write-Host "Retrying performance report in Docker fallback..."
    $reportArgs = Get-ReportArgs
    $quotedArgs = $reportArgs | ForEach-Object { Convert-ToBashSingleQuoted $_ }
    $featureArgs = if ($EnableLsmRocksdb) { " --features lsm-rocksdb" } else { "" }
    $buildCommand = "export PATH=/usr/local/cargo/bin:`$PATH; cargo build --package gitgraph-lab-core --release --locked --bin perf-report$featureArgs"
    $runCommand = "export PATH=/usr/local/cargo/bin:`$PATH; /cargo-target/release/perf-report " + ($quotedArgs -join " ")

    $buildResult = Invoke-DockerPhase `
        -RepoRootPath $RepoRootPath `
        -CommandText $buildCommand `
        -PhaseLabel "Docker build phase (perf-report binary)..."
    if ($buildResult.ExitCode -ne 0) {
        throw "Performance report build failed in Docker fallback."
    }

    $executeResult = Invoke-DockerPhase `
        -RepoRootPath $RepoRootPath `
        -CommandText $runCommand `
        -PhaseLabel "Docker execute phase (perf-report workload)..."
    if ($executeResult.ExitCode -ne 0) {
        throw "Performance report execution failed in Docker fallback."
    }

    $totalSeconds = Format-ElapsedSeconds($buildResult.Duration + $executeResult.Duration)
    Write-Host ("Docker build phase duration (s): " + (Format-ElapsedSeconds $buildResult.Duration))
    Write-Host ("Docker execute phase duration (s): " + (Format-ElapsedSeconds $executeResult.Duration))
    Write-Host ("Docker total duration (s): " + $totalSeconds)
    Write-Host "Performance report completed (Docker fallback)."
}

Push-Location $repoRoot
try {
    $previousIngestPrimary = $env:GITGRAPH_INGEST_PRIMARY
    if ($EnableLsmRocksdb) {
        $env:GITGRAPH_INGEST_PRIMARY = "rocksdb"
    }

    if ($EnableLsmRocksdb -and -not [string]::IsNullOrWhiteSpace($LibclangPath)) {
        $env:LIBCLANG_PATH = $LibclangPath
    }

    if ($ProbeOnly) {
        $cargoCommand = Get-Command "cargo" -ErrorAction SilentlyContinue
        if ($null -eq $cargoCommand) {
            throw "cargo was not found in PATH while probing performance report script."
        }
        Write-Host "Performance report probe passed."
        Write-Host "Home: $userHome"
        Write-Host "Cargo target: $CargoTargetRoot"
        return
    }

    if (Test-EnvEnabled -Name "GITGRAPH_PERF_REPORT_DOCKER_ONLY") {
        if ($EnableLsmRocksdb) {
            throw "GITGRAPH_PERF_REPORT_DOCKER_ONLY is incompatible with -EnableLsmRocksdb. Use host execution with -LibclangPath."
        }
        Invoke-PerfReportDocker -RepoRootPath $repoRootPath
        return
    }

    $reportArgs = Get-ReportArgs
    $hostBuildArgs = @(
        "build",
        "--package",
        "gitgraph-lab-core",
        "--release",
        "--locked",
        "--bin",
        "perf-report"
    )
    $hostBuildArgs += Get-CargoFeatureArgs
    $repoLocalTargetRoot = Join-Path $repoRootPath "target"

    $buildResult = Invoke-ExternalPhase `
        -Executable "cargo" `
        -Arguments $hostBuildArgs `
        -PhaseLabel "Host build phase (perf-report binary)..."

    $combinedOutputText = $buildResult.OutputText
    if ($buildResult.ExitCode -ne 0 -and -not $cargoTargetRootWasExplicit) {
        $currentTargetNormalized = Normalize-AbsolutePath $CargoTargetRoot
        $repoTargetNormalized = Normalize-AbsolutePath $repoLocalTargetRoot
        if ($currentTargetNormalized -ne $repoTargetNormalized) {
            Write-Host (
                "Host build failed with cargo target '" + $CargoTargetRoot +
                "'. Retrying with repo-local cargo target '" + $repoLocalTargetRoot + "'..."
            )
            Set-CargoTargetRoot -NextCargoTargetRoot $repoLocalTargetRoot
            $buildResult = Invoke-ExternalPhase `
                -Executable "cargo" `
                -Arguments $hostBuildArgs `
                -PhaseLabel "Host build phase retry (repo-local target)..."
            $combinedOutputText = $buildResult.OutputText
        }
    }

    if ($buildResult.ExitCode -ne 0) {
        if ((Test-IsPolicyBlocked $combinedOutputText) -and (Test-DockerReady) -and -not $EnableLsmRocksdb) {
            Invoke-PerfReportDocker -RepoRootPath $repoRootPath
            return
        }
        if ((Test-IsPolicyBlocked $combinedOutputText) -and $EnableLsmRocksdb) {
            throw "Performance report build is policy-blocked on host, and Docker fallback is unavailable for -EnableLsmRocksdb."
        }
        throw "Performance report build failed."
    }

    $binaryName = if ($env:OS -eq "Windows_NT") { "perf-report.exe" } else { "perf-report" }
    $binaryPath = Join-Path (Join-Path $CargoTargetRoot "release") $binaryName
    if (-not (Test-Path $binaryPath)) {
        throw "Performance report binary not found after build: $binaryPath"
    }

    $executeResult = Invoke-ExternalPhase `
        -Executable $binaryPath `
        -Arguments $reportArgs `
        -PhaseLabel "Host execute phase (perf-report workload)..."
    $combinedOutputText = ($combinedOutputText + [Environment]::NewLine + $executeResult.OutputText).Trim()
    if ($executeResult.ExitCode -ne 0) {
        if ((Test-IsPolicyBlocked $combinedOutputText) -and (Test-DockerReady) -and -not $EnableLsmRocksdb) {
            Invoke-PerfReportDocker -RepoRootPath $repoRootPath
            return
        }
        if ((Test-IsPolicyBlocked $combinedOutputText) -and $EnableLsmRocksdb) {
            throw "Performance report execution is policy-blocked on host, and Docker fallback is unavailable for -EnableLsmRocksdb."
        }
        throw "Performance report execution failed."
    }

    $totalSeconds = Format-ElapsedSeconds($buildResult.Duration + $executeResult.Duration)
    Write-Host ("Host build phase duration (s): " + (Format-ElapsedSeconds $buildResult.Duration))
    Write-Host ("Host execute phase duration (s): " + (Format-ElapsedSeconds $executeResult.Duration))
    Write-Host ("Host total duration (s): " + $totalSeconds)
    Write-Host "Performance report completed."
}
finally {
    if ($null -eq $previousIngestPrimary) {
        Remove-Item Env:GITGRAPH_INGEST_PRIMARY -ErrorAction SilentlyContinue
    } else {
        $env:GITGRAPH_INGEST_PRIMARY = $previousIngestPrimary
    }
    Pop-Location
}

