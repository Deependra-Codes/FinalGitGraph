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
    [switch]$PreconditionCommitGraph,
    [switch]$KeepFixture,
    [switch]$ProbeOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$repoRootPath = $repoRoot.Path
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
        try {
            $ErrorActionPreference = "Continue"
            & $Executable @Arguments 2>&1 | Tee-Object -FilePath $phaseLog.FullName
            $exitCode = $LASTEXITCODE
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

    return @(
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
        "PATH=/usr/local/cargo/bin:/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "-w",
        "/work",
        "rust:1.94",
        "bash",
        "-c",
        $CommandText
    )
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
    if ($PreconditionCommitGraph) {
        $args += "--precondition-commit-graph"
    }
    if ($KeepFixture) {
        $args += "--keep-fixture"
    }
    return $args
}

function Invoke-PerfReportDocker {
    param([string]$RepoRootPath)

    Write-Host "Retrying performance report in Docker fallback..."
    $reportArgs = Get-ReportArgs
    $quotedArgs = $reportArgs | ForEach-Object { Convert-ToBashSingleQuoted $_ }
    $buildCommand = "cargo build --package gitgraph-lab-core --release --locked --bin perf-report"
    $runCommand = "/cargo-target/release/perf-report " + ($quotedArgs -join " ")

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

    $buildResult = Invoke-ExternalPhase `
        -Executable "cargo" `
        -Arguments $hostBuildArgs `
        -PhaseLabel "Host build phase (perf-report binary)..."

    $combinedOutputText = $buildResult.OutputText
    if ($buildResult.ExitCode -ne 0) {
        if ((Test-IsPolicyBlocked $combinedOutputText) -and (Test-DockerReady)) {
            Invoke-PerfReportDocker -RepoRootPath $repoRootPath
            return
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
        if ((Test-IsPolicyBlocked $combinedOutputText) -and (Test-DockerReady)) {
            Invoke-PerfReportDocker -RepoRootPath $repoRootPath
            return
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
    Pop-Location
}
